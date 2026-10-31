# RAM pools: audit and design

Design input for a memory manager made of a few static pools with explicit
tenants (not a heap). Step 1 lists every static of 200 B or more, who
touches it and when it is live; then which buffers can share bytes, what
that frees for the scope record (`scope::CAP`), and the pool API.

Base: release/0.4.0 at fe59a782, native default build. Memory map, link
checks and earlier moves:
[native-boot.md, SRAM audit](native-boot.md#sram-audit-2026-10-30-featsram-overlays),
[Extra RAM areas](native-boot.md#extra-ram-areas-2026-10-30-featsram-extra-areas),
[ram-budget.md](ram-budget.md) (who is live when; its table is from
2026-10-24: since then `scope_ui::GRAPH` grew 9 956 -> 16 380 B and
`scope_page::CACHE` 4 824 -> 8 024 B for the GEN and CH2 traces, and
`scope_ch2::STORE` (8 776 B) is new).

| build.sh figure | release/0.4.0 | test/scope-chain-0.4 (dd9d1021) |
|---|---|---|
| `scope::CAP` | 26 624 (BUF 53 248 B) | 43 008 (BUF 86 016 B) |
| SRAM free above `.sram_bss` (link floor 8 KiB) | 45 460 B | 10 660 B |
| stack (TCM above `.bss`) / static worst path / headroom (floor 20 480) | 45 320 / 24 488 / 20 832 B | 45 900 / 24 824 / 21 076 B |
| backup SRAM free / USBHS RAM free | 2 008 / 0 B | 936 / 0 B |

## 1. Result

| candidate | kind | bytes freed (SRAM / TCM / backup) | scope CAP gain (release) | chain gain | risk |
|---|---|---|---|---|---|
| **A. CAP into today's free SRAM** (no pool) | static | uses 32 768 B SRAM | 26 K -> **42 K** samples, always (44 K fits the default build, not recovery-usb) | +1 K (its SRAM is spent) | low: the chain has run 42 K |
| **B. Record tail** over `scope_ch2::STORE` + `logger::RING` (+ `csvlog::FIFO_BUF`) | dynamic, scope acquisition = restartable tenant | 0 static; the record grows while those tenants are idle | +12.6 K (STORE + RING) / +16.7 K (+ FIFO) samples while CH2, every AD2 stream (and recording / fast log / MMEM:CAT? cache) are off | the same | high: DMA ring vs stream ISR, a stored record that shrinks, WAV:DATA? stream |
| **C. Page pool**: scope graph state with the gallery, the generator preview and the touch test | static, droppable / rebuildable tenants | 484 / 672 / 168 | +242 samples | +242 | low |
| **D. Job pool**: the working parts of the fast-log, CSV and scope-CSV jobs | static | ~1 000 (mixed SRAM / TCM, *estimated*) | +~250 | +~250 | medium: job results are read after the job (LOG:CSV?) |

Implemented (2026-10-31):

| candidate | PR | result |
|---|---|---|
| A | [#200](https://github.com/opendm40/opendm40/pull/200) | CAP 43 008 (42 K), not 44 K: the recovery-usb build has 4 096 B less SRAM free and keeps 8 596 B at 42 K (floor 8 192) |
| C | [#201](https://github.com/opendm40/opendm40/pull/201) | SRAM +480 B, TCM (stack headroom) +632 B, region B -744 B; `src/pool.rs`, DBG:POOL? (`dbg-pool`), tools/pool-test, emu/pooltest.py |

Ranking: A, then B (needs the owner: section 6 Q1), then C (small; its value
is the pool framework and the checked compatibility table, which B needs
anyway), then D. The owner's premise "many buffers are never live
together" holds mostly for the small ones: every SRAM buffer over 4 KiB
can be live while the scope page is open (section 3).

## 2. Inventory

Sizes: `llvm-nm -S` of the native ELF. Access: **M** main loop, **ISR** the
AD2 stream ISR (`logger::ad2_stream_irq`, IRQ 55; the only interrupt the
firmware enables), **DMA** with its controller / stream, **USB** the
usb-device core (polled from the main loop: `dev.poll`, no USB interrupt).
Lifetime terms: *always*; *page X* (the view); *activity* (section 3).
"Survives" = content needed after its page / mode ends; "rebuild" = cost
to recreate it.

### SRAM (0x2000_0000, `.dmabuf` NOLOAD then `.sram_bss`; DMA-reachable)

| static | B | module | access | lifetime | survives? / rebuild | constraints |
|---|---|---|---|---|---|---|
| `scope::BUF` | 53 248 | scope.rs | M; DMA1 S4 (fs >= 50 MS/s GPIOB IDR) / S3 (ADC) circular | always while the scope rail is on (acquisition in every view that powers it; Scope, Gen, combined view, a host scope command, generator on); the stored record while stopped (any view); lent to the inrush page (`lend_buf`); read in place by a streamed WAV:DATA? (`host_read` holds the scope stopped) and by a scope CSV save (`scope_csv`) | stored record: yes until the next RUN / lend; acquisition: restartable (one record) | DMA-reachable; the DMA must be halted (`halt`) before anyone else writes it; NDTR is 16-bit (<= 65 535 samples) |
| `scope::LATCH` | 8 | scope.rs | DMA1 | acquisition | no | DMA |
| `scope::ROLL_COLS` | 1 600 | scope.rs | M | roll mode (from 200 ms/div), any view while the scope polls; read by `App::scope_poll` for WAV:DATA? screen columns | no: refilled (`fill(MAX)`) when roll starts | none |
| `scope::WAVE` | 224 | scope.rs | M | wave view state + MCX settings kept during it | yes (settings) | none |
| `scope_wav::SCREEN` | 800 | scope_wav.rs | M (per frame; read per USB packet) | always: the last frame's 400 columns for WAV:DATA? (SCReen) | no: next frame | none |
| `scope_ui::GRAPH` | 16 380 | scope_ui.rs | M | page Scope / combined view / wave view: trace, persistence, math, GEN, CH2 rows (`tr ps ma gn c2`, 1 596 B each) and their on-screen copies (`d_*`, 5 x 1 596) | no (*inferred*): rebuilt from `CACHE` + the next frame on a full repaint; `sp_leave` does not clear it | none (CPU only) |
| `scope_ui::SHOWN` | 344 | scope_ui.rs | M | page Scope / combined view | no: invalidated on repaint | none |
| `scope_ui::ROWS` | 320 | scope_ui.rs | M | one `Graph::flush` | no: per flush | none |
| `scope_page::CACHE` | 8 024 | scope_page.rs | M | page Scope / combined view: frame column rows (`seg`, `gseg`, `c2seg`, 1 600 B each) and persistence layers `ps_a` / `ps_b` (1 596 each) | persistence: kept over a page change today (`sp_leave` leaves it; cleared only in roll / STOP / persist off); losing it = infinite persistence restarts (owner Q3) | none |
| `scope_ch2::STORE` | 8 776 | scope_ch2.rs | M (`ch2_drain` from the logger ring) | page Scope with CH2 on (`ch2_want`: `View::Scope` only, not the combined / wave view); after a STOP it holds the stored record's CH2 and `ch2_rows` draws from it while CH2 is on, also after leaving and re-entering the page (*inferred* from `ch2_rows` / `ch2_want`: no reset until the next stream start) | stopped + CH2 on: yes (not rebuildable); running: no (`reset` at stream start); its scale (`scale().amps`) is read for the label | none |
| `logger::RING` | 16 540 | logger.rs | ISR producer (AD2 stream) or M producer (AD1, `no-ad2-isr`); M consumer | an AD2 / AD1 stream: SCPI LOG:STATe ON (any view), a fast-log session, CH2, the wave view, the inrush capture | records are drained by LOG:DATA? / fastlog / the views; after `stop` nothing reads them except LOG:DATA? of a host session (*inferred*: "Captured data stays readable" in main.rs) | written by the ISR: never revocable while a stream runs; atomics only |
| `csvlog::FIFO_BUF` | 8 192 | csvlog.rs | M | CSV recording (`fifo()`); fast-log session (`lend_fifo`); otherwise its first 6 144 B (`CAT_MAX`) are the MMEM:CAT? cache, warmed before every stream start | cache: droppable (`ok` cleared, rescan ~10 block reads); recording rows: until committed | none |
| `diskbuf::ARENA` | 12 288 | diskbuf.rs | M | the disk arena: every disk job, the FFT work buffer, the strip renderer ([ram-budget.md](ram-budget.md#the-disk-buffer-arena-diskbufarena-12-kib)) | no; FFT job: restartable (`GEN`) | none |
| `bt::RX` | 1 024 | bt.rs | DMA0 ch2 circular (UART4 RX) + M | BT powered | no | DMA; not revocable while BT is on |
| `bt::TX` | 6 656 | bt.rs | DMA0 ch4 + M | BT connected: one whole reply + stock frames | until sent | DMA, whole reply ([SRAM audit](native-boot.md#sram-audit-2026-10-30-featsram-overlays)) |
| `gen::WAVE` | 4 000 | gen.rs | DMA0 S5 circular (DAC) | generator output on (any view) | the table: rebuilt from the settings (`genplan`) | DMA; not revocable while on |
| `vdisk::TAB` | 6 420 | vdisk.rs | M (USB sector reads) | the generated USB volume is exported (vdisk builds: always after USB enumerates) | regenerates on reads | none |
| `vwrite::W0` | 1 376 | vwrite.rs | M (every pass, every sector) | writable-volume session | session state | none |
| `fastlog::JOB` | 1 864 | fastlog.rs | M | fast-log session; its result after the end (fast logger status) | result: yes; `enc taken ext buf hdr` (~1 630 B): session only (*inferred* from the field names) | none |
| `fastvos::FAST` | 216 | fastvos.rs | M | always (DMM) | yes | none |
| `ui::GEN_SP` | 484 | ui.rs | M | page Gen (strip passes of `gen_preview`) | no: key word 0 = recompute | none |
| `powermeter::PW` | 576 | powermeter.rs | M | always (DMM state, power meter in any view) | yes | none |
| `acdcm::AM` | 480 | acdcm.rs | M | always (AC+DC method) | yes | goes behind `acdc-meth` (section 5) |
| `stockfont::data::LUT` | 512 | stockfont_data.rs | M | always (glyph decode) | constant | none |
| HY3131 frames `TXI RXI TXB RXB`, `logger::isr::*` | < 200 each | hy3131.rs, logger.rs | ISR, DMA0 ch0 / ch7 | AD2 stream | no | DMA + ISR |

### TCM (0x1000_0000, `.data` / `.bss`; not DMA-reachable; shares 64 KiB with the stack)

| static | B | module | access | lifetime | survives? |
|---|---|---|---|---|---|
| `APP` | 7 056 | main.rs | M | always (below) | yes |
| `scope_page::FFT` | 4 128 | scope_page.rs | M | the last spectrum (`db` 4 100 B) and the job in flight; scope page FFT view | yes: kept when a job is dropped ([ram-budget.md](ram-budget.md#the-disk-buffer-arena-diskbufarena-12-kib)) |
| `dmm::REC_COPY` | 1 024 | dmm.rs | M | always (settings / calibration record copy) | yes |
| `dlog::CSV` | 2 x 952 | dlog.rs | M (per host byte) | host reads of the generated CSV files and MMEM:DATA? of them (one pool, #197) | per file read; restartable |
| `lcd::DEFER` | 712 | lcd.rs | M | always (draw path) | no |
| `gallery::G` | 672 | gallery.rs | M | page Gallery (`poll` only there; `open` resets the list) | no: `open` rebuilds (*inferred*) |
| `scope_csv::SNAP` | 548 | scope_csv.rs | M | one scope CSV save (one-shot, then released by `sp_csv_poll`, any view) | no |
| `MSC_IO`, `MSC_BLK` | 512 each | main.rs | USB (MSC class, polled) | always | no |
| `csvlog::JOB` | 512 | csvlog.rs | M | CSV recording; its result after the end (LOG:CSV?) | result: yes |
| `EP_MEMORY` | 256 | main.rs | USB core (USBFS endpoint buffers) | always | no |
| `scope_page::AS_LOG` | 256 | scope_page.rs | M | AUTO-set decision log, read by DBG:GEST? | debug data |
| `inrush::SES` | 232 (`.data`) | inrush.rs | M | the inrush page's session incl. its threshold setting | settings: yes |

`APP` fields of 64 B or more (DWARF member offsets): `dmm` 1 672 (DMM
state), `chart` 1 284 (DMM strip chart: fed in every view except Gallery /
Inrush, so not page-local), `meter` 896, `chart_cache` 728, `scope` 456,
`sp` 456 (scope page state), `dmm_view` 236, `gest_log` 192, `last_raw` /
`prov` 168 each, `hist` / `meas_res` 80 each. Nothing here is page-local
and large; `chart` + `chart_cache` (2 012 B) are the only candidates and
they run in the background.

### Backup SRAM (0x4002_4000) / USBHS FIFO RAM (0x4006_0000, word stores only)

All CPU-only, never on the ISR's paths (build.sh check), see
[Extra RAM areas](native-boot.md#extra-ram-areas-2026-10-30-featsram-extra-areas).

| static | B | area | lifetime |
|---|---|---|---|
| `persist::RECORD_SPACE` | 292 | backup | the settings record (never accessed; reserves the bytes) |
| `screenshot::JOB` | 452 | backup | a screenshot / scope record write; `busy` / `frozen` flags every pass |
| `vdisk::STATE` | 372 | backup | always (USB volume) |
| `upload::UP` | 368 | backup | an upload / firmware staging |
| `scope_csv::PLAN` | 264 | backup | one scope CSV save |
| `app_touchtest::TT` | 168 | backup | page Touch test |
| `csvlog::TAIL` | 4 096 | USBHS | CSV recording commits |

## 3. Activities and the concurrency matrix

Background activities run on any page. "x" = can be live together (the
code does not refuse it), "-" = excluded by the code, with the reason
under the table.

| | scope page (Scope / combined) | CH2 | wave view | inrush page | gallery | Gen page | CSV rec | fast log | LOG:STAT ON | stored record + WAV:DATA? stream | BT | USB volume | disk job (shot, commit, MMEM:CAT?, upload) | FFT | gen output |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| scope page | | x | x (1) | - | - | - | x | x | x | x | x | x | x | x | x |
| CH2 | | | - (2) | - | - | - | - (3) | - (4) | - (4) | x | x | x | x (5) | x | x |
| wave view | | | | - | - | - | - (6) | - | - (7) | x | x | x | x (5) | x | x |
| inrush page | | | | | - | - | - (6) | x (8) | - (7) | - (9) | x | x | x | - | x |
| CSV rec | | | | | x | x | | - | x | x | x | x | x | x | x |
| fast log | | | | | x | x | | | - (10) | x | x | x | x | x | x |
| LOG:STAT ON | | | | | x | x | | | | x | x | x | x | x | x |

1. The wave view is the Scope view with source DMM (same GRAPH).
2. `ch2_set` refuses CH2 in the wave view; `ch2_rows` empty there.
3. `ch2_why`: "CH2: recording runs"; a recording stops a running CH2.
4. main.rs `ch2_want` filter: no CH2 while `logger.host_active()` or `fastlog::active()`.
5. A screenshot pauses the stream (`screenshot::busy`), the view stays.
6. `App::rec_conflict` (app.rs): the wave view and inrush refuse to start during a recording.
7. `take_wave_req` / `take_inrush_req`: "Data logger running".
8. The inrush save is a fast-log session whose records are the captured trace (`inrush_saving`).
9. `lend_buf` drops the stored record.
10. A fast-log session and LOG:STATe ON share the logger (`fast_events`).

Buffers per activity (SRAM, B):

| activity | buffers it needs live |
|---|---|
| scope acquisition / stored record | BUF 53 248, ROLL_COLS 1 600 (roll), SCREEN 800 |
| scope page | + GRAPH 16 380, CACHE 8 024, SHOWN 344, ROWS 320; TCM FFT 4 128 |
| CH2 | + STORE 8 776, RING 16 540 |
| wave view | GRAPH, CACHE, BUF (wave record from the ring), RING |
| inrush page | BUF (lent), RING; not GRAPH / CACHE / STORE |
| CSV recording | FIFO_BUF 8 192, TAIL 4 096 (USBHS), ARENA during commits, csvlog JOB |
| fast log | RING, FIFO_BUF, ARENA, fastlog JOB 1 864 |
| LOG:STAT ON | RING |
| BT | RX 1 024, TX 6 656 |
| USB volume | TAB 6 420, MSC_IO / MSC_BLK, `dlog::CSV` on CSV reads, W0 1 376 in writable sessions |
| disk job / FFT / strip repaint | ARENA 12 288 (shared today) |
| gen output | WAVE 4 000 |

Worst case on the scope page (CH2 on, recording refused then; BT, USB, a
disk job, gen output): BUF + GRAPH + CACHE + STORE + RING + ARENA +
FIFO_BUF (cache) + TX/RX + TAB + WAVE = every big buffer. So no two SRAM
buffers over 4 KiB can share bytes statically, except STORE with
the CSV recording's FIFO_BUF use, which fails on the MMEM:CAT? cache that
CH2 streams rely on (`catalog_warm` before `start_wave`).

## 4. Pool candidates

### A. CAP into today's free SRAM (not a pool)

45 460 - 8 192 = 37 268 B would allow CAP 44 K (45 056 samples) in the
default build, but the recovery-usb variant has 4 096 B less SRAM free:
CAP 42 K (43 008 samples, +32 768 B; recovery-usb keeps 8 596 B). Take the CAP / `INRUSH_LEN` / `lin_len` part of the chain's
aada110b (inrush keeps a 26 K ring; a linear record is no longer in time
than max(26 K samples, 0.1 s); `scope_csv::REC_MAX` 65 536 covers it).
The chain is at 10 660 B free: +1 K samples.

### B. Record tail (dynamic)

Place the CH2 store, the logger ring and the row FIFO right after BUF in
one `#[repr(C)]` SRAM struct. The scope's DMA ring runs over BUF plus the
idle prefix of that struct:

| tail tenant (in order) | B | evict rule | live while |
|---|---|---|---|
| `Ch2Store` | 8 776 | never while CH2 is on and stopped (stored CH2); else droppable | CH2 on |
| `Ring` | 16 540 | never while a stream runs (ISR producer) | any AD2 / AD1 stream |
| `Fifo` | 8 192 | recording / fast log: never; MMEM:CAT? cache: droppable | recording, fast log, cache valid |
| `ScopeTail` | the idle prefix | restartable (acquisition) / droppable (a stored record is cut to the base length) | no tenant above |

- Gain: CAP_live = 44 K + 12.6 K (Store + Ring idle) or + 16.7 K (and Fifo
  idle), capped by the 16-bit NDTR at 65 535 (stock Single: 65 520).
- Eviction: every start path of a tenant is in main.rs (logger `start*`,
  `ch2_started`, `csvlog::fifo`, `lend_fifo`, `catalog_warm`), so one call
  `pool.take(Tenant::Ring)` before them halts the scope DMA (`halt`), cuts
  the stored record and restarts the acquisition at the shorter length.
- Risks: the DMA must be stopped and drained before the ISR's first push
  (ordering in main.rs); `host_read` (a WAV:DATA? stream reading the tail)
  must refuse or delay the tenant start; a scope CSV save holding the
  record (`scope_csv::SNAP.rec`) likewise; every `CAP` use in scope.rs
  (~40) becomes a runtime length; `rotate_left` of a stopped ring touches
  the tail; the inrush lend stays on the base part.

### C. Page pool (static)

| tenant | B | area today | evict rule | rebuild |
|---|---|---|---|---|
| `ScopeGraph` (GRAPH + SHOWN + ROWS) | 17 044 | SRAM | rebuildable | `graph().invalidate()` + full repaint from CACHE (a page entry repaints anyway) |
| `Gallery` (`gallery::G`) | 672 | TCM | rebuildable | `open()` |
| `GenPreview` (`ui::GEN_SP`) | 484 | SRAM | rebuildable | key word 0 -> recomputed on the next strip pass |
| `TouchTest` (`app_touchtest::TT`) | 168 | backup | rebuildable | page entry |

Size = 17 044 (ScopeGraph). Tenants conflict only through the view: one
view at a time; the scope page owns `ScopeGraph` from `sp_enter` (and the
wave view), the others from their page entries. Leaving CACHE and STORE
out keeps persistence and the stored CH2 over page changes as today.
Saves 484 B SRAM, 672 B TCM, 168 B backup. Risk: anything drawing the
graph off-page (`scope_poll` only draws when `sp_on()`; `ch2_set` clears
`graph().c2` from the Display tab, a scope page).

### D. Job pool (static)

Split the session-only working state from the result in `fastlog::JOB`
(~1 630 of 1 864 B, *inferred*), `csvlog::JOB` (`taken` 128, `enc` 80),
`scope_csv::SNAP` (548, TCM) and the backup-SRAM jobs, then overlay the
working parts: a fast-log session and a CSV recording exclude each other
(`fastlog::begin`); a scope CSV save is a disk job that waits for the
disk (*inferred*: shares `screenshot::step`). ~1 000 B; touches the most
code per byte saved.

### Not candidates

| buffer | why |
|---|---|
| `logger::RING` with `csvlog::FIFO_BUF` | fast log drains one into the other |
| `bt::TX` / `RX`, `gen::WAVE` | DMA sources / sinks while on; on at any time |
| `vdisk::TAB` | host reads at any time |
| `scope_page::CACHE` | persistence survives page changes (owner Q3) |
| `APP.chart` | fed in the background |
| GRAPH's per-feature rows (math, GEN, CH2: 3 x 3 192 B) | all three can be on together |

## 5. Design

### API (`src/pool.rs`)

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Tenant { ScopeGraph, Gallery, GenPreview, /* ... */ }

pub enum Evict { Never, Drop, Restart }   // per tenant, const table

pub struct Spec { pub t: Tenant, pub off: usize, pub len: usize, pub evict: Evict }

pub struct Pool<const N: usize> {
    mem: UnsafeCell<[u32; N]>,      // word-aligned bytes
    owner: AtomicU32,               // bit per tenant holding bytes
    gen: [AtomicU32; T],            // bumped when a tenant loses its bytes
    specs: &'static [Spec],
}

impl Pool {
    /// The tenant's bytes as T, evicting overlapping holders whose rule
    /// allows it (Drop / Restart: their gen moves); None while a `Never`
    /// holder overlaps.
    pub fn take<T: Tenancy>(&self, t: Tenant) -> Option<&'static mut T>;
    /// Still mine since `gen`? (a tenant keeps its gen across passes.)
    pub fn holds(&self, t: Tenant, gen: u32) -> bool;
    pub fn give(&self, t: Tenant);
}
```

- Main loop only (as every tenant today); `owner` / `gen` are atomics so
  a DBG query reads them. `take` costs an atomic load, a mask test and a
  store; no `RefCell`, no large `Option` on the stack (stack headroom is
  352 B over the floor).
- Type safety: `Tenancy` is implemented per tenant type with
  `const T: Tenant`; `take` checks `size_of::<T>() <= spec.len` and the
  alignment at compile time (`const` assert per impl).
- Eviction: `Drop` / `Restart` holders are told by their `gen` changing
  (the pattern of `calbackup::fft_work` / `GEN`); `Restart` tenants
  (the record tail, the FFT job) redo their work; `Drop` tenants
  (caches, rebuildable page state) re-init on their next `take`.
- DBG:POOL? behind the cargo feature `dbg-pool` (as `dbg-touch`,
  `dbg-prof`): per pool the owner mask and every tenant's gen.

### Compatibility table and host test

One const table per pool: the specs (offset, length, rule) and
`COEXIST: &[(Tenant, Tenant)]`, the pairs the code lets be live together
(section 3). The host test (`cargo test` in `tools/ci-tests.sh`) fails when

- two tenants that are in `COEXIST` share a byte;
- a `Never` tenant overlaps any tenant not listed as excluded by the code
  (documents why each overlap is safe);
- a spec runs past the pool or a tenant type exceeds its spec.

### Existing ad-hoc sharing folded in

| today | as tenants |
|---|---|
| `diskbuf::ARENA`: disk users / `fft_work` / `strip_take` with `GEN`, `disk_job` | disk pool: `Disk` (Never while `disk_job()`), `FftWork` (Restart), `Strip` (Drop, inside one call) |
| `csvlog::FIFO_BUF`: `fifo()` / `lend_fifo()` / the MMEM:CAT? cache | `Rec`, `FastLog` (Never), `CatCache` (Drop: `ok` cleared) |
| `scope::BUF` lent to inrush | `ScopeRec` (Restart) vs `InrushRing` (Never while the page is open) |

### Concurrent changes by other agents (not repeated here)

| change | effect on this plan |
|---|---|
| the disk arena moved into its own module (`diskbuf::ARENA`, #198) | the disk pool above is that module's tenants |
| `src/acdcm.rs` behind the feature `acdc-meth` | `acdcm::AM` (480 B SRAM) leaves the default build: +240 samples for A |
| `dlog::SCPI_GEN` + `vdisk::CSV_CUR` / `CSV_CUR2` as one 2-cursor pool `dlog::CSV` (#197) | ~950 B TCM (stack headroom), not SRAM |
| slow USB with scope CH2 | none on the layout; B's `Ch2Store` eviction must not add work on the CH2 path |

## 6. Open questions

| # | question | for |
|---|---|---|
| Q1 | B: may the record length depend on what else runs (shorter while CH2, a stream, a recording)? A stopped record would be cut when one of them starts. | owner |
| Q2 | B: a WAV:DATA? stream or a scope CSV save of a long record when LOG:STATe ON arrives: refuse the logger ("Settings conflict") or cut the transfer? | owner |
| Q3 | Must infinite persistence (`CACHE.ps_a/ps_b`) survive a page change? Today it does (by omission in `sp_leave`, *inferred* unintentional or not). | owner |
| Q4 | Does the stored record's CH2 have to survive leaving the Scope view? Today it does (*inferred*). | owner |
| Q5 | CAP 44 K: rotation of a stopped 88 KB ring and the larger WAV:DATA? MAX transfer on the device (time per STOP, USB throughput) | device test |
| Q6 | C: a Scope -> Gen -> Scope swipe and Gallery -> back repaint the graph completely (no stale `d_*` diff) | emulator + device |
