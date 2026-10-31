/*
 * DM40 replacement firmware, NATIVE format: the app image the FS-loader
 * installs at 0x08004000 (stock update-file body), executed in place (XIP;
 * the GD32F4 low 512 KiB of flash is zero-wait, so nothing is copied to SRAM).
 *
 * Flash (body offset = address - 0x08004000):
 *   A  0x08004000..0x08008000  16 KiB  vector table + shim (.vectors, .shim),
 *                                      then region A2 (.a2) up to the marker:
 *                                      zero-wait flash in the update body's
 *                                      sector 1 (FS-loader writes it like any
 *                                      body byte), named hot read-only data
 *                                      (`.rodata.a2.*`: the ASCII26, F14,
 *                                      LETTERS, UNIT, LOCK font blobs,
 *                                      tools/mkstockfont.py A2_FONTS).
 *                                      ~14.8 KB; the link fails
 *                                      when it reaches the marker, build.sh
 *                                      warns below 2 KiB free.
 *      0x08008000..0x08008010  16 B    GAP, 0xFF in the binary. The ROM and the
 *                                      FS-loader read AND write a 12-byte record
 *                                      at 0x08008000..0x0800800B (excluded from
 *                                      the update body CRC by dm40fw.py); the
 *                                      next 4 bytes are padding.
 *   B  0x08008010..0x08080000  480 KiB .text, .rodata: everything that may
 *                                      run or be read while the AD2 stream
 *                                      ISR runs. The linker fails when it
 *                                      does not fit.
 *   C  0x08080000..0x080A0000  128 KiB .cold (boot-only / rarely run code:
 *                                      calibration backup, fatren, the
 *                                      gallery, screenshots, the DLG writer,
 *                                      FAT / vdisk generation, uploads, the
 *                                      bus self-tests (DBG),
 *                                      scope CSV, boot / DBG:BOOT?). Must
 *                                      end below 0x080A0000 (FS-loader, UID
 *                                      data, calibration above). The .data
 *                                      load image follows .text in B (read
 *                                      once by the reset copy; in C until
 *                                      region C audit #1).
 *   Why the split: flash from 0x08080000 up is NOT zero-wait on the GD32F4
 *   (only the low 512 KiB are). A read there stalls the bus for ~20 us, and
 *   with it the AD2 stream ISR's entry (device c50425f: max_lat 18.8-21.8
 *   us whenever the logger panel drew ASCII26 glyphs, which had grown past
 *   0x08080000; 4.6 us without the panel; docs/dmm-input-scope-feasibility.md
 *   11.4). Code in C costs that per call: only what never runs while a
 *   stream does (or seldom, outside the main loop's every-pass path).
 *   DATA loads too, from anywhere >= 0x08080000 (region C's rodata, the
 *   calibration record, the loader data): device c92b605, the USB volume's
 *   INFO/ files, 21.8 us per load. None while a stream runs (RAM copies,
 *   the volume answers NOT READY; docs/native-boot.md "The slow flash bank
 *   while an AD2 stream runs"; emu/regionc.py watches the loads).
 *
 * RAM:
 *   TCM     0x10000000  64 KiB  .data, .bss, stack (top 0x10010000)
 *   SRAM    0x20000000 192 KiB  .dmabuf (DMA buffers, scope record),
 *                               .sram_bss
 *   BKPSRAM 0x40024000          backup SRAM after the settings record
 *                               (.bkp_record, persist.rs) up to its
 *                               sequence words at 0x40024FE8: .bkp_bss
 *   USBHSRAM 0x40060000  4 KiB  the unused USBHS core's FIFO RAM: .usbhs_bss
 *   Both: CPU only (no DMA), ~5 cycles a load, never touched by the AD2
 *   stream ISR, zeroed at reset by native.rs extra_ram_init (docs/
 *   native-boot.md "Extra RAM areas").
 */
MEMORY
{
    FLASH_A (rx) : ORIGIN = 0x08004000, LENGTH = 0x4000
    FLASH_B (rx) : ORIGIN = 0x08008010, LENGTH = 0x08080000 - 0x08008010
    FLASH_C (rx) : ORIGIN = 0x08080000, LENGTH = 0x080A0000 - 0x08080000
    TCM (rw)     : ORIGIN = 0x10000000, LENGTH = 64K
    SRAM (rw)    : ORIGIN = 0x20000000, LENGTH = 192K
    /* Ends at persist.rs BKP_SEQ (the sequence + boot words above). */
    BKPSRAM (rw) : ORIGIN = 0x40024000, LENGTH = 0x40024FE8 - 0x40024000
    USBHSRAM (rw) : ORIGIN = 0x40060000, LENGTH = 4K
}

ENTRY(reset);
EXTERN(__VECTORS);

_stack_top = ORIGIN(TCM) + LENGTH(TCM);       /* 0x10010000 */
/* Word[0] of the image: only the FS-loader's `msr msp` before `bx reset` uses
 * it (reset moves SP to _stack_top at once). Same value as the stock app so
 * any check the loader applies to it passes as for stock. */
_loader_sp = 0x2002F7F8;
_marker_start = 0x08008000;
_marker_end = 0x0800800C;

SECTIONS
{
    .vectors ORIGIN(FLASH_A) :
    {
        __image_start = .;
        KEEP(*(.vectors));
        KEEP(*(.shim .shim.*));
        . = ALIGN(4);
        __region_a_end = .;
    } > FLASH_A

    /* Region A2: the rest of FLASH_A after the shim, below the loader marker
     * (zero-wait, part of the update body). Claimed before .cold / .text
     * (first matching rule wins). Only hot, B-class data or code may go
     * here (it is as fast as B); nothing the shim / recovery needs (that
     * stays in .vectors / .shim). */
    .a2 :
    {
        __a2_start = .;
        *(.rodata.a2 .rodata.a2.*);
        *(.text.a2 .text.a2.*);
        . = ALIGN(4);
        __a2_end = .;
    } > FLASH_A

    /* Claimed before .text (the first matching rule wins), placed in C by
     * its region. Module paths in the v0-mangled section names:
     * <len><name> after the crate (8opendm40). */
    .cold ORIGIN(FLASH_C) :
    {
        __cold_start = .;
        *(.text.*8opendm409calbackup*);
        *(.text.*8opendm406fatren*);
        /* dmm.rs load_record (the record copy and its CRC, once at
         * start-up, before any stream) and rec_crc_ok. */
        *(.text.*8opendm403dmm11load_record*);
        *(.text.*8opendm403dmm10rec_crc_ok*);
        *(.text.*8opendm406native*);
        *(.text.*8opendm403uid4init*);
        /* recovery::run only: the rest of recovery.rs (the recovery-usb /
         * recovery-file variants' own code, ~1 KB) goes to region B, which
         * those variants have room in, and keeps their region C above its
         * 4 KiB floor; the default image has only `run`. */
        *(.text.*8opendm408recovery3run*);
        *(.text.*8opendm407gallery*);
        *(.text.*8opendm4010screenshot*);
        *(.text.*8opendm404dlog*);
        *(.text.*8opendm406fatdir*);
        /* The settings file (boot, load, the writer); its per-pass check
         * setfile::poll is inline(always) into main.rs `run`: region B. */
        *(.text.*8opendm407setfile*);
        *(.text.*8opendm405vdisk*);
        *(.text.*8opendm406upload*);
        *(.text.*8opendm405hyspi*);
        /* NOT ad2exp (DBG:AD2:TORN? / NOISE? / DCV? / N7?, E7's dc_enter
         * and dcv_row) nor main.rs dbg_dcv / dbg_n7 and scpi::dcv_arg /
         * n7_arg: region B again since region C audit #3 (2026-10-29, C at
         * 7.7 KB free, B at 50 KB; docs/native-boot.md). They were in B
         * from region C audit #1 (133f81e) and in C from region B audit #4
         * (2026-10-25, B at 15 KB). */
        *(.text.*8opendm409scope_csv*);
        /* The decoder's frame log: a frame's waveform, drawn on a tap, the
         * top bar / counters, the page draw, the setup popup and the
         * view's touches (app_log.rs `.text.log_cold`; ui_log.rs's panel
         * buttons below). The log streams by DMA (the decode catches up
         * from its ring, log_poll / log_tick / log_row stay in B). It
         * never runs with the AD2 stream: `log_open` refuses while one
         * runs, LOG:STAT ON is refused while the log is open (main.rs
         * log_on_check), and every other stream start (Wave view, inrush,
         * the fast logger's page) leaves View::Decode first. */
        *(.text.log_cold*);
        *(.text.*8opendm408scope_ui3log12panel_button*);
        *(.text.*8opendm408scope_ui3log9panel_hit*);
        /* The keypad's digit font: read only by ui::keypad_key (above, in
         * C: the Keypad page / popup, no stream). */
        *(.rodata.fonts.K_FONT_E);
        *(.rodata.fonts.K_FONT_B);

        /* NOT the scope WAVeform:* / ACQ:SRAT? / TRIG:STAT? / CHAN1:OFFS?
         * replies (src/scope_wav.rs) nor their parser scpi::wav_cmd: a host
         * polls them while the Wave view's stream runs (device c830e09:
         * max_lat 6.16 us at scope_wav::query; emu/hy3131/wavetest.py
         * polls them under the region C hooks). Region B, ~2 KB. */
        *(.text.*8opendm404boot*);
        *(.text.*8opendm403app9app_press*);
        /* Inrush current: open / arm / close / results / save glue and the
         * full page (src/app_inrush.rs; app_inrush_live.rs and the page's
         * live parts stay in region B: they run while the AD2 stream does). */
        *(.text.*8opendm403app10app_inrush*);
        /* The touch test page (src/app_touchtest.rs, src/ui_touchtest.rs):
         * never open while the AD2 stream or a logger session runs (the page
         * refuses to open then, closes when one starts; main.rs calls
         * tt_poll only without a session). */
        *(.text.*8opendm403app13app_touchtest*);
        *(.text.*8opendm402ui9touchtest*);
        /* The touch calibration's fit and checks (src/touchcal.rs): the
         * touch test page, DBG:TOUCH:CAL and the settings restore at boot
         * (the per-frame apply is touch.rs `screen`, region B). */
        *(.text.*8opendm408touchcal*);
        *(.text.*8opendm404scpi8tcal_arg*);
        /* The Record page's SCPI settings (LOG:CSV:INTerval / DURation /
         * CHANge): parse and set / query, rarely sent. */
        *(.text.*8opendm404scpi7csv_arg*);
        *(.text.*8opendm408rec_scpi*);
        *(.text.*8opendm402ui3inr4page*);
        *(.text.*8opendm402ui3inr5trace*);
        *(.text.*8opendm406inrush*3Cap7results*);
        *(.text.*8opendm406inrush*3Cap9linearize*);
        *(.text.*8opendm406inrush*3Cap7records*);
        *(.text.*8opendm406inrush*3Cap3arm*);
        /* dlog's per-reading row coder is `.text.dlog_row` (src/dlog.rs),
         * not matched here: region B. So is `.text.vdisk_rd`: the USB
         * volume's per-sector read path (VDisk::read_block, the FAT / data
         * sectors, a FAST CSV's generator: Gen::read, the kind-3 decoder,
         * FatFile, flash_read; 2026-10-22: per 512-byte sector, ~6 KB). A
         * host reading the volume during a stream then costs the ISR
         * nothing, and the CSV is not generated from slow flash. upload::disk / spi and
         * vdisk::entries are inline(always) (vwrite::poll, every pass);
         * app_press's guard is inlined into App::gesture. Audit with
         * emulator hooks on every function of C, 2026-10-22 (TODO). */
        /* The fast logger's Prep / Stop (fastlog::prep: before / after the
         * logger session, never while the AD2 ISR streams); its Run and
         * fastenc.rs stay in B. */
        *(.text.*8opendm407fastlog4prep*);
        *(.text.*8opendm409fast_head*);
        *(.text.*8opendm4010fast_start*);
        *(.text.*8opendm408fast_req*);
        *(.text.*8opendm4012log_on_check*);
        *(.text.*8opendm4010stale_read*);
        /* NOT fast_scpi (LOG:DEST / LOG:DURation, set and query): region B.
         * Device c92b605: LOG:DEST? / LOG:DUR? in the Wave view at 76800
         * gave late 1, max_lat 2.8 / 5.8 us at lat_pc fast_scpi. */
        *(.text.*8opendm4012fast_next_hz*);
        *(.text.*8opendm407fastlog5begin*);
        *(.text.*8opendm407fastlog10plan_bytes*);
        *(.text.*8opendm4011fast_events*);
        *(.text.*8opendm402ui11record_page*);
        *(.text.*8opendm403app*3App11record_page*);
        *(.text.*8opendm403app*3App9fast_rate*);
        /* A tap on the DMM value: the rate step and its toast text (the
         * rate chip's rate_chip / rps_text stay in B, drawn every chart
         * pass). */
        *(.text.*8opendm402ui10rate_toast*);
        *(.text.*8opendm403app*3App16resolution_cycle*);
        *(.text.*8opendm403app*3App15record_fast_tap*);
        *(.text.*8opendm403app*3App16record_page_fast*);
        *(.text.*8opendm403app*3App10fast_ended*);
        *(.text.*8opendm403dmm*3Dmm7wd_slow*);
        /* Burden voltage SCPI (FETC:BURD?, DISP:BURD, POW:BURD's shunt). */
        *(.text.*8opendm403app*3App11burden_scpi*);
        *(.text.*8opendm403app*3App12burden_shunt*);
        *(.text.*8opendm403dmm*3Dmm9burden_of*);
        /* "Leads in A?" reminder's beep / toast (once per function change;
         * the per-pass check App::leads_tick stays in B). */
        *(.text.*8opendm403app*3App12leads_remind*);
        *(.text.*8opendm403app*3App11leads_toast*);
        /* NOT persist: its crc runs on every main-loop pass (device
         * 00e088f: lat_pc in persist::crc, 9..18 us stalls). */
        /* ... and their named read-only data (tables, strings in statics). */
        *(.rodata.*8opendm409calbackup*);
        *(.rodata.*8opendm406fatren*);
        *(.rodata.*8opendm406native*);
        *(.rodata.*8opendm408recovery*);
        *(.rodata.*8opendm407gallery*);
        *(.rodata.*8opendm4010screenshot*);
        *(.rodata.*8opendm404dlog*);
        *(.rodata.*8opendm406fatdir*);
        *(.rodata.*8opendm407setfile*);
        *(.rodata.*8opendm405vdisk*);
        *(.rodata.*8opendm406upload*);
        *(.rodata.*8opendm405hyspi*);
        *(.rodata.*8opendm409scope_csv*);
        *(.rodata.*8opendm404boot*);
        *(.rodata.*8opendm403app9app_press*);
        *(.rodata.*8opendm407fastlog4prep*);
        *(.rodata.*8opendm403app13app_touchtest*);
        *(.rodata.*8opendm402ui9touchtest*);
        *(.rodata.*8opendm408touchcal*);
        /* Single functions (and their closures) that run only at start-up or
         * on a user's settings / page action, never per pass nor in the ISR:
         * the App construction (main.rs new_app, App::new and the sub-struct
         * constructors inlined there), the settings / keypad / limits /
         * temperature / help pages, the generator page's static parts (not its
         * 1 s status), the fuse-check dialog, the power page's entry. A
         * pattern that stops matching (a rename) leaves the function in B,
         * the safe side. NOT the generator's plan (gen::plan) and Gen::apply
         * (~3.7 KB): a SOUR:* / OUTP setting from a host may come during an
         * AD2 stream (fastlogtest --region-c --host-read --poll 'SOUR:FREQ
         * 1000;...'), so they stay in B. */
        *(.text.*8opendm407new_app*);
        *(.text.*8opendm403app*3App11fuse_dialog*);
        *(.text.*8opendm403app*3App11limits_page*);
        *(.text.*8opendm403app*3App7kp_open*);
        *(.text.*8opendm403app9app_power*3App13pw_open_entry*);
        /* The power page after a coupling change (once per change). */
        *(.text.*8opendm403app9app_power*3App15pw_coup_changed*);
        *(.text.*8opendm403app9app_power*3App15pw_pending_draw*);
        *(.text.*8opendm402ui8settings*);
        *(.text.*8opendm402ui9temp_page*);
        /* Its info line, once a second while the temperature page is open
         * (no AD2 stream in temperature). Region B audit 2026-10-30, ~0.45 KB:
         * room for ui::ol_word's "OPEN" / "Overflow" in B. */
        *(.text.*8opendm403app*3App14temp_info_text*);
        *(.text.*8opendm403app*3App14temp_info_tick*);
        *(.text.*8opendm402ui8temp_key*);
        *(.text.*8opendm402ui10gen_values*);
        *(.text.*8opendm402ui10gen_static*);
        *(.text.*8opendm402ui14auto_cpl_label*);
        /* The ohm / diode test-current label: drawn on a function / range /
         * variant change only (no AD2 stream runs in ohm / diode). */
        *(.text.*8opendm402ui18test_current_label*);
        *(.text.*8opendm402ui10range_page*);
        /* Pressed feedback of the MF range / Power Settings pages (per touch). */
        *(.text.*8opendm402ui11range_press*);
        *(.text.*8opendm402ui2pw14settings_press*);
        /* Debug / file replies, `#[inline(never)]` so they do not land in
         * main.rs `run` (hot): DBG:TOUCH?, DBG:DIAG?, MMEM:DEL (and the
         * modules above: DBG:HYSPI, DBG:UPLOAD, MMEM:DATA?).
         * Not DBG:PROF? (prof::report, ~1 KB) nor MMEM:CAT? (csvlog::
         * catalog_scpi with fatren's / fatdir's `.text.fat_rd` helpers)
         * nor CONF?'s
         * App::range_si: region B, asked during fast sessions / the Wave
         * view (device 33b44bd: DBG:PROF? 12 us, a sample lost at 38400;
         * emu/fastlogtest.py --region-c --host-read, emu/hy3131/
         * wavetest.py's whole-run region C check). Nor LOG:SCALe?
         * (main.rs log_scale: a host scales the session's data with it;
         * fastlogtest --poll, 2026-10-23: region C code and its literal
         * loads while streaming). */
        /* NOT the SCPI range / resolution commands (scpi::range_cmd,
         * conf_cmd, range_arg, res_arg, step_exp, pow10f, App::scpi_range;
         * ~2 KB): back in region B since region C audit #2 (2026-10-25,
         * docs/native-boot.md "Region C audit #2"). ui::help_page went back
         * with them and returned here with the DMM accuracy label (0.6.0,
         * the same section): drawn on the Help pages only, where no stream
         * runs (the list below). */
        *(.text.*8opendm405touch8log_text*);
        *(.text.*8opendm4010touch_diag*);
        *(.text.*8opendm406csvlog6delete*);
        /* ... their disk bodies (the `_on` variants and their closures). */
        *(.text.*8opendm406csvlog9delete_on*);
        /* Tap / page-only code that sat inlined in hot callers (region B
         * audit 2026-10-22 #2): the temperature page's draw / key helper
         * (App::with_temp_page), the scope's Save CSV
         * (App::sp_save_csv, with scope_csv::set and the DLG block plan). */
        *(.text.*8opendm403app*3App14with_temp_page*);
        *(.text.*8opendm403app*3App11sp_save_csv*);
        /* scope_ui::drag_target (once per scope drag) and
         * Scope::hw_calibrate are back in region B (2026-10-26, the long
         * scope record: region C had < 8 KiB, B had room after
         * stockfont::paint stopped being one copy per closure). */
        /* Region B audit #3 (2026-10-22, 11.9 KB free on 1ad38aa):
         * `#[inline(never)]` event / start-up code that sat inlined in hot
         * callers. App::wave_open runs at event rate. (App::tap with
         * pw_tap / kp_apply / range hits, App::key and App::show were here
         * too; back in B since region A2 took the ASCII26 / F14 font blobs
         * out of B, 2026-10-22: App::tap meets a stream on the inrush
         * page.) Start of a recording session (App::rec_start with
         * csvlog::start). NOT Logger::start / logger::isr::start: back in
         * B (2026-10-23): their tails run after stream::start, with the ISR
         * on (the slow-bank data watch, emu/regionc.py, caught
         * Logger::start's literal load of DWT_CYCCNT's address).
         * Page / dialog draws on a view change (power
         * settings page (not its per-tick settings_status), device page,
         * fuse dialog). DBG / calibration replies (DBG:ACDC?, DBG:SENT?,
         * SCOP:CAL with HwTrig::calibrate). Start-up only: Touch::new,
         * Bt::new (not init_module: polled every pass), SpiFlash::new,
         * usb::init_hw. */
        *(.text.*8opendm403app*3App9wave_open*);
        /* NOT App::sp_popup_close / sp_uncover, scope_ui::uncover (~0.3 KB,
         * region B since 2026-10-30): an MF popup open in the Wave view is
         * closed by MENU while the AD2 stream runs (hy3131/wavetest.py
         * "exit (MENU)"). */
        /* DBG:IDLE? (emu/idletest.py; no stream runs in its tests). */
        *(.text.*8opendm403app*3App8dbg_idle*);
        /* DBG:GENTRIG:LAG / STAT? and DBG:CH2:LAG (the GEN / CH2 channels'
         * tuning replies, App::dbg_gen). */
        *(.text.*8opendm403app*3App7dbg_gen*);
        /* Start-up only (App::new): the scope's SHOWN / STRIP_SHOWN reset
         * (scope_ui::init_shown) and the backup-SRAM record set-up
         * (persist::init, also splash / setfile::boot). */
        *(.text.*8opendm408scope_ui10init_shown*);
        *(.text.*8opendm407persist4init*);
        /* DBG:OHM6 (the 600 Mohm range's tuning, dmm.rs Ohm600). */
        *(.text.*8opendm403dmm*3Dmm10ohm600_dbg*);
        *(.text.*8opendm403app*3App9rec_start*);
        *(.text.*8opendm402ui2pw13settings_page*);
        *(.text.*8opendm402ui11device_page*);
        *(.text.*8opendm406dialog10fuse_check*);
        *(.text.*8opendm403dmm*3Dmm14acdc_diag_text*);
        *(.text.*8opendm403dmm*3Dmm8reg_text*);
        *(.text.*8opendm403dmm*3Dmm12precondition*);
        /* acdcm.rs: the AC+DC methods' entry / range-change / SCPI steps
           (poll_am, the per-result path, stays in region B) */
        *(.text.*8opendm403dmm2am*8am_enter*);
        *(.text.*8opendm403dmm2am*11am_set_meth*);
        *(.text.*8opendm403dmm2am*11am_set_zero*);
        *(.text.*8opendm403dmm2am*7am_zero*);
        *(.text.*8opendm403dmm2am*7am_stat*);
        *(.text.*8opendm403dmm2am*7am_mode*);
        *(.text.*8opendm403dmm2am*6am_row*);
        *(.text.*8opendm403dmm2am*6am_ref*);
        *(.text.*8opendm403dmm2am*11am_cal_step*);
        *(.text.*8opendm408sentinel*8Sentinel9diag_text*);
        /* The jack check (sentinel.rs JACK CHECK): only while PB1 hits in ohm /
         * continuity / the resistive temperature sensors (a fault), never
         * with the bus handed to a stream (it talks to the HY3131 itself). */
        *(.text.*8opendm408sentinel*8Sentinel11jack_result*);
        *(.text.*8opendm408sentinel*8Sentinel6v_both*);
        *(.text.*8opendm403dmm*3Dmm9cont_jack*);
        /* V_GATE of ohm / continuity / diode, once per reading (no HY3131
         * access; region B at 16 KiB). */
        *(.text.*8opendm403dmm*3Dmm10v_side_ohm*);
        /* AC mV's V_GATE, once per AC mV result (no HY3131 access). */
        *(.text.*8opendm403dmm*3Dmm8mv_vside*);
        /* DBG:SRC? and DBG:ACDC:TRACE? reply text (SCPI only; room in region B
         * for the jack check above). */
        *(.text.*8opendm40*8src_text*);
        /* DBG:SRC's parser (region B at 16 KiB, 2026-10-26). */
        *(.text.*8opendm406dbgsrc5index*);
        *(.text.*6acdctr*5Trace4text*);
        *(.text.*8opendm406hwtrig*6HwTrig9calibrate*);
        *(.text.*8opendm405touch*5Touch3new*);
        *(.text.*8opendm402bt*2Bt3new*);
        *(.text.*8opendm408spiflash*8SpiFlash3new*);
        *(.text.*8opendm403usb7init_hw*);
        /* USB endpoint allocation (usb-device / synopsys-usb-otg): only
         * when main.rs `run` builds the USBTMC / MSC classes, before its
         * loop; enumeration and resets do not allocate. Region B audit #5
         * (2026-10-26, fix/low-freq). */
        *(.text.*16synopsys_usb_otg3bus*8alloc_ep*);
        *(.text.*16synopsys_usb_otg3bus*17EndpointAllocator*5alloc*);
        *(.text.*10usb_device3bus*15UsbBusAllocator*5alloc*);
        *(.text.*10usb_device3bus*15UsbBusAllocator*4bulk*);
        /* Menu / list / keypad page helpers (already their own symbols):
         * drawn or hit-tested only on the Menu, Settings, Record, Limits,
         * Keypad, Help and Device pages, where no stream runs (leaving the
         * Wave view or the inrush page stops theirs; touch is off while
         * the logger runs). NOT ui::main_menu_hit: a DBG:TAP on the menu
         * during a fast-log session reached it (fastlogtest --ui-nav;
         * region B since 2026-10-23, ~0.2 KB). */
        *(.text.*8opendm402ui9menu_item*);
        *(.text.*8opendm402ui9help_page*);
        *(.text.*8opendm402ui9list_page*);
        *(.text.*8opendm402ui10page_title*);
        *(.text.*8opendm402ui11set_stepper*);
        *(.text.*8opendm402ui12keypad_input*);
        *(.text.*8opendm402ui10keypad_key*);
        *(.text.*8opendm402ui10keypad_hit*);
        *(.text.*8opendm403app*3App9kp_labels*);
        *(.text.*8opendm403app*3App8lim_unit*);
        /* The generator page's boxes and its 1 s status (Gen / Keypad page
         * only: no stream runs there) and vwrite's temp-file purge (the
         * first pass after a start, a host write session's purge). */
        *(.text.*8opendm402ui7gen_box*);
        *(.text.*8opendm402ui10gen_status*);
        *(.text.*8opendm402ui13gen_box_frame*);
        *(.text.*8opendm402ui16gen_box_value_bg*);
        *(.text.*8opendm402ui12gen_wave_box*);
        *(.text.*8opendm402ui12gen_box_note*);
        *(.text.*8opendm406vwrite9purge_tmp*);
        /* ... and its post-write steps (a host wrote a file on the volume;
         * MSC writes are refused while a logger session runs): the update
         * file's park / check / finalize / commit. */
        *(.text.*8opendm406vwrite4park*);
        *(.text.*8opendm406vwrite5check*);
        *(.text.*8opendm406vwrite8finalize*);
        *(.text.*8opendm406vwrite6commit*);
        /* Region B audit #8 (2026-10-30, test/scope-chain-0.4 on 0.4.0 with
         * GEN / CH2): functions whose every caller is region C code (a call
         * graph from the ELF, every address-taken function counted as hot):
         * the generator page's wave preview (ui::gen_values), vwrite's
         * staging of a host write (VDisk::write_block), the scope CSV's DLG
         * sample coder (App::sp_save_csv), the gallery's DLG decoders,
         * HwTrig::calibrate's sweep, the limits / keypad / record page
         * helpers. */
        *(.text.*8opendm402ui11gen_preview*);
        *(.text.*8opendm406vwrite5stage*);
        *(.text.*8opendm406vwrite9ours_live*);
        *(.text.*4dlog*4SEnc4next*6pack105Codes*);
        *(.text.*4dlog*4SEnc3put*);
        *(.text.*7gallery4GSrcE*);
        *(.text.*8opendm406csvlog5ascii*);
        *(.text.*8opendm402ui21ok_and_limits_buttons*);
        *(.text.*8opendm406hwtrig*6HwTrig5sweep*);
        *(.text.*8opendm402ui2pw7outline*);
        *(.text.*8opendm402ui13record_button*);
        *(.text.*8opendm403app12kp_unit_text*);
        *(.text.*8opendm408spiflash9block_crc*);
        /* Struct templates copied only by region C code (fastlog::begin,
         * csvlog::start from App::rec_start, App::tt_open). */
        *(.rodata.cold_tpl*);
        . = ALIGN(4);
        __cold_end = .;
    } > FLASH_C

    .text ORIGIN(FLASH_B) :
    {
        __stext = .;
        *(.text .text.*);
        *(.rodata .rodata.*);
        . = ALIGN(4);
        __etext = .;
    } > FLASH_B

    .data :
    {
        . = ALIGN(4);
        __sdata = .;
        *(.data .data.*);
        . = ALIGN(4);
        __edata = .;
    } > TCM AT > FLASH_B
    __sidata = LOADADDR(.data);
    /* End of region B's content: .text / .rodata, then the .data image. */
    __b_end = LOADADDR(.data) + SIZEOF(.data);

    .bss (NOLOAD) :
    {
        . = ALIGN(4);
        __bss_start = .;
        *(.bss .bss.* COMMON);
        . = ALIGN(4);
        __bss_end = .;
    } > TCM

    .dmabuf (NOLOAD) :
    {
        . = ALIGN(4);
        *(.dmabuf .dmabuf.*);
        *(.sram .sram.*);
    } > SRAM

    /* Zero-initialised CPU-only statics kept out of TCM (the stack's
     * memory): `#[link_section = ".sram_bss"]` on an all-zero static; zeroed
     * at reset next to .bss (native.rs init_ram, main.rs rust_start). */
    .sram_bss (NOLOAD) :
    {
        . = ALIGN(4);
        __sram_bss_start = .;
        *(.sram_bss .sram_bss.*);
        . = ALIGN(4);
        __sram_bss_end = .;
    } > SRAM

    /* The settings record's place (persist.rs RECORD_SPACE), then the
     * CPU-only buffers (`#[link_section = ".bkp_bss"]`). */
    .bkp_record (NOLOAD) :
    {
        __bkp_record_start = .;
        KEEP(*(.bkp_record));
        __bkp_record_end = .;
    } > BKPSRAM

    .bkp_bss (NOLOAD) :
    {
        . = ALIGN(4);
        __bkp_bss_start = .;
        *(.bkp_bss .bkp_bss.*);
        . = ALIGN(4);
        __bkp_bss_end = .;
    } > BKPSRAM

    .usbhs_bss (NOLOAD) :
    {
        . = ALIGN(4);
        __usbhs_bss_start = .;
        *(.usbhs_bss .usbhs_bss.*);
        . = ALIGN(4);
        __usbhs_bss_end = .;
    } > USBHSRAM

    __image_end = __cold_end;

    /DISCARD/ :
    {
        *(.ARM.exidx*);
        *(.ARM.extab*);
        *(.comment);
    }
}

ASSERT(__region_a_end <= _marker_start, "region A (vectors + shim) overlaps the loader marker at 0x08008000");
ASSERT(__a2_start == __region_a_end, "region A2 does not start right after the shim");
ASSERT(__a2_end <= _marker_start, "region A2 (.rodata.a2.*) overlaps the loader marker at 0x08008000: move a font out of A2_FONTS (tools/mkstockfont.py)");
ASSERT(ADDR(.text) >= _marker_end, "code placed in the loader marker at 0x08008000..0x0800800B");
ASSERT(__image_end <= 0x080A0000, "image reaches the FS-loader at 0x080A0000");
/* Region C budget: >= 4 KiB free below 0x080A0000 (build.sh warns below
 * 8 KiB). C grows by whole moved functions (0.3..5 KB each);
 * 4 KiB still takes one more small move after the warning. */
ASSERT(0x080A0000 - __image_end >= 4K, "less than 4 KiB of region C (.cold) free below 0x080A0000: cut code or move .cold code back to region B");
/* Memory budgets (2026-10-22 memory rework; the numbers then, ad2-isr (the
 * default) / without it: static worst stack path 21 760 / 21 396 B, stack
 * 46 620 / 46 688 B, SRAM free 12 856 / 13 040 B, region B free 24 508 /
 * 31 884 B).
 *   Stack = TCM above .bss. .data/.bss may not take it below 42 KiB (the
 *   worst path + the 20 KiB build.sh requires, with ~4 KiB for statics);
 *   build.sh also checks the static worst call path itself
 *   (tools/stack-depth/stackdepth.py --check 20480), which catches frame
 *   growth this assert cannot see. (Before: measured high-water 28 412 B,
 *   emulator 2026-10-21, + 16 KiB.)
 *   SRAM: >= 8 KiB free above .sram_bss (the last SRAM section) for DMA
 *   descriptors / buffers still to come.
 *   Region C: >= 4 KiB free below 0x080A0000 (build.sh warns below 8 KiB;
 *   nothing above 0x080A0000 is within the update file's reach).
 *   Region B: >= 8 KiB free below 0x08080000 (build.sh ends with a warning
 *   banner below 16 KiB; hot code / fonts only, rarely run code goes to
 *   .cold above). After the fast logger 9 304 / 16 568 B were left; the
 *   region B audit #2 (2026-10-22: cold-module code that had been inlined
 *   into hot callers, Gallery::poll into App::tick above all, and the
 *   f32 quicksort of poll_cap) brought it to 30 368 / 37 840 B; region C
 *   then has 31 700 / 31 764 B left below 0x080A0000 (was 48 364 / 48 428).
 *   Audit #3 (on 1ad38aa: 11 928 / 20 672 B): 26 160 / 32 808 B; region
 *   C 10 256 / 12 352 B left (docs/native-boot.md "Region B audit #3").
 *   Region A2 (2026-10-22, on 06011ac: B 26 136 / 33 320, C 10 120 /
 *   12 240 B): ASCII26 + F14 blobs in A2 (2 652 B left; the link fails at
 *   the marker, build.sh warns below 2 KiB), App::tap / key / show and
 *   vdisk::describe back in B: B 30 572 / 37 836, C 17 840 / 19 880 B
 *   (docs/native-boot.md "Region A2"). */
/* Extra RAM areas: never over the settings record or its sequence words. */
ASSERT(__bkp_record_start == 0x40024000, "persist.rs RECORD_SPACE is not at the start of the backup SRAM");
ASSERT(__bkp_record_end > __bkp_record_start, "persist.rs RECORD_SPACE missing: .bkp_bss would overlay the settings record");
ASSERT(__bkp_bss_start >= __bkp_record_end, ".bkp_bss starts inside the settings record");
ASSERT(__bkp_bss_end <= 0x40024FE8, ".bkp_bss reaches the settings sequence words at 0x40024FE8: move a buffer out");
ASSERT(__usbhs_bss_start >= 0x40060000 && __usbhs_bss_end <= 0x40061000, ".usbhs_bss outside the 4 KiB USBHS FIFO RAM");
ASSERT(_stack_top - __bss_end >= 42K, "TCM stack below 42 KiB (worst path ~21.5 KiB + 20 KiB headroom): move CPU-only statics to .sram_bss");
ASSERT(ORIGIN(SRAM) + LENGTH(SRAM) - __sram_bss_end >= 8K, "less than 8 KiB of SRAM free: overlay or shrink a buffer (docs/native-boot.md memory map)");
ASSERT(0x08080000 - __b_end >= 8K, "less than 8 KiB of zero-wait flash (region B) free: move rarely run code to .cold");
