#![no_std]
#![no_main]

mod app;
mod bargraph;
mod bt;
mod clock;
mod danger;
mod dmm;
mod fastvos;
mod gen;
mod gesture;
mod msc;
mod spiflash;
mod hy3131;
mod hy3131_tables;
mod ident;
mod hyspi;
mod lcd;
mod logger;
mod inrush;
mod ad2exp;
mod ad2pll;

/// The critical-section implementation: compiler fences only, no PRIMASK.
/// opendm40 runs with interrupts masked except while the AD2 stream ISR runs
/// (decision #3), and that ISR touches nothing a `critical_section::with`
/// protects (synopsys-usb-otg's endpoint buffers and register RMWs, the
/// only users): masking it there only delayed it (device b48048e: max_lat
/// 4.7 us, every worst case inside Endpoint<Out>::read). Main-loop code
/// shares state with the ISR through atomics only (logger.rs RING).
struct NoMask;
critical_section::set_impl!(NoMask);
unsafe impl critical_section::Impl for NoMask {
    unsafe fn acquire() -> critical_section::RawRestoreState {
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
    unsafe fn release(_: critical_section::RawRestoreState) {
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}
mod meter;
mod mmio;
mod persist;
mod power;
mod prof;
mod scope;
mod hwtrig;
mod scope_meas;
mod scope_ui;
mod scope_ch2;
mod fft;
mod sinc;
mod trigsoft;
mod scpi;
mod scpi_msg;
mod sentinel;
#[cfg_attr(not(feature = "dbg-src"), allow(dead_code))]
mod dbgsrc;
mod stockfont;
mod touch;
mod touchcal;
mod ui;
mod usb;
mod usbtmc;
mod wdt;
mod wordram;

use core::panic::PanicInfo;

// Links the single-core critical-section implementation the USB stack needs.
use cortex_m as _;

use usb_device::prelude::*;

use clock::Clock;

/// System control block: application interrupt and reset control.
const SCB_AIRCR: u32 = 0xE000_ED0C;

/// Watchdog policy (see the end of this file): the main loop feeds the FWDGT
/// until a deadline, INITIAL_HOLD_MS after start (in effect: for as long as
/// the loop runs), pushed forward by host requests. After DBG:RELEASE, a panic
/// or a hang the watchdog (~20 s) resets, the shim counts a crash and boots
/// opendm40 again.
const INITIAL_HOLD_MS: u64 = u64::MAX / 4;
const KEEPALIVE_MS: u64 = 15_000;
/// SCPI session / transport index (scpis, scpi_wait).
const USB: usize = 0;
const BT: usize = 1;
const KEEPALIVE_MAX_MS: u64 = 600_000;
/// MEASure:<f>? after a function change waits for a full averaging window at
/// most this long (then any fresh reading answers), ms. A full window
/// (dmm::AVG_FULL = 8 conversions at OSR 32768, 400 ms each) needs ~3.2 s
/// after the switch to the steady OSR, which comes ~2 s after the row write
/// (dmm.rs R22_LATE): ~5.3 s. So on DC V / A at MED / SLOW the answer
/// is the fresh reading at this limit (an OSR 8192 one, no +1 digit); a
/// later READ? has the digit. Kept short for the 2 s PyVISA default timeout
/// (decided 2026-10-14, TODO "SLOW reports every conversion").
const SCPI_SETTLE_MS: u64 = 1500;
/// READ? / MEASure? / FETCh? with no reading of the current function yet
/// (after a function change): give up this long after the converter's settle
/// deadline (`Dmm::deadline`), not after the command, ms. AUTO coupling:
/// AUTO_WAIT_MS.
const SCPI_WAIT_MS: u64 = 1900;
/// READ? / MEASure:<f>? with a reading of the current function already
/// flowing: the next fresh reading if it comes within this, else the latest
/// one, ms. libsigrok's USBTMC transport reads Bulk-IN with a hard 1000 ms
/// timeout; the expiry is checked once per main-loop iteration, hence the
/// margin. Fits the slowest DC conversion (OSR 32768 chopped, 400 ms: the
/// steady MEDIUM and SLOW reading, both one per conversion) with room for
/// the poll.
const SCPI_QUICK_MS: u64 = 850;
/// READ? / MEASure? in the AUTO coupling after the host's change: wait for
/// the first decision until this long after the coupling was entered (its
/// values before it are provisional, switch-latency.md 6: ~1.3 s on DC,
/// ~3.5 s on a clear AC signal), later queries at most SCPI_QUICK_MS; then
/// the part AUTO shows by then (the DC part of a tie, whose decision can take
/// ~10 s behind the AC part's settle), not "Data stale". libsigrok's DM40
/// profile reads 3 s after CONF with a hard 1 s USBTMC timeout: < 4 s.
const AUTO_WAIT_MS: u64 = 3900;
// The next fresh steady reading: at most one conversion away + poll / loop.
const _: () = assert!(SCPI_QUICK_MS >= hy3131::ad1_period_us_r22(0x17) as u64 / 1000 + 300);
/// scpi_wait expiry of a READ? / MEAS? / FETCh? whose function changed under
/// it, not by its own host (touch, keys, phone app, DBG:TAP / KEY, the other
/// transport): it answers at once with +9.91E37 (SCPI NaN) and "Data stale".
/// The host asked FUNCtion? before it and would file a reading of the new
/// function under the old one; nothing of the old function will come.
const WAIT_ORPHAN: u64 = 0;
const _: () = assert!(dialog::LOG_PANEL_ROOM == logger::CAP as u32 / 4);

/// READ? / FETCh?: true (the reply +9.91E37 and "Data stale" put) when the
/// host's FUNCtion? / CONFigure? since its last READ? named another function
/// than `func` (scpi::take_asked). Region C (link-native.x): once per READ?.
#[inline(never)]
fn stale_read(scpi: &mut scpi::Scpi, func: u8) -> bool {
    if !scpi::take_asked(scpi).is_some_and(|a| a != scpi::func_id(func)) {
        return false;
    }
    scpi.len = 0;
    scpi.put_bytes(scpi::NAN);
    scpi.set_err("Data stale");
    true
}

/// The DMM function as FUNCtion? names it, with the temperature sensor: the
/// key of a function change for the READ? cap (`func_key`, `host_key`).
fn scpi_func_key(d: &dmm::Dmm) -> (u8, u8) {
    (scpi::func_id(d.func()), d.t_sensor())
}

/// pid.codes VID 1209, PID D040 for opendm40 (owner 2026-10-25; the pid.codes
/// request is filed once the repository is public). Builds before 0.1.0 used
/// the pid.codes test PID 0001; the host tools accept both.
const USB_VID_PID: UsbVidPid = UsbVidPid(0x1209, 0xD040);

// CPU-only USB buffers in TCM .bss (2026-10-22 memory budget: the stack
// has the headroom now, SRAM is the tighter one; they were .sram_bss).
/// OUT-endpoint packet buffers of synopsys-usb-otg (`allocate_rx_buffer`:
/// one max-packet buffer per OUT endpoint; the TX FIFOs live in the core).
/// USBFS has 4 endpoints and a full-speed packet is at most 64 bytes, so 4 x
/// 16 words is the most any configuration can take (today: EP0 + the USBTMC
/// and MSC bulk OUT). Was 320 words.
static mut EP_MEMORY: [u32; 64] = [0; 64];
static mut MSC_IO: [u8; 512] = [0; 512];
/// The MSC block buffer (was a 4 KiB field of `Msc` on the main stack).
/// 512 bytes in TCM; the 4 KiB of the builds whose disk is the SPI flash
/// (recovery-usb, no `vdisk`) go to SRAM (the TCM stack keeps its 42 KiB).
#[cfg_attr(all(target_os = "none", any(feature = "recovery-usb", not(feature = "vdisk"))), link_section = ".sram_bss")]
static mut MSC_BLK: [u8; msc::MAX_BLOCK] = [0; msc::MAX_BLOCK];
/// Data-logger state (FR-38; its 16 KiB ring is `logger::RING`, shared with
/// the AD2 stream ISR). In SRAM (.dmabuf, NOLOAD: `Logger::init_in_place` in
/// main()), not TCM .bss, which the stack shares.
#[cfg_attr(target_os = "none", link_section = ".dmabuf")]
static mut LOGGER: logger::Logger = logger::Logger::new();

/// DIAG block size (DBG:DIAG?).
#[cfg_attr(not(feature = "dbg-diag"), allow(dead_code))]
const DIAG_LEN: usize = 212;

/// Bring-up failures as red squares at the top left, one slot per stage.
fn fail(slot: u16, failed: bool) {
    if failed {
        lcd::fill_rect(8 + slot * 28, 40, 20, 20, lcd::RED);
    }
}

const ST_PLLSAI: u16 = 0;
const ST_CORE: u16 = 1;

fn main() -> ! {
    wdt::kick();
    screenshot::init(); // JOB and LOGGER are in NOLOAD .dmabuf
    logger::Logger::init_in_place(core::ptr::addr_of_mut!(LOGGER), &logger::RING);
    let mut clk = Clock::new();
    lcd::set_madctl(lcd::MADCTL_LANDSCAPE);
    // The boot splash (native.rs pre_main) stays up until the first frame
    // (`run`).

    // The settings / calibration record's RAM copy, before anything reads
    // it (dmm.rs REC_COPY: the record is in the slow flash bank).
    dmm::load_record();
    // Slow init (relays, touch reset ~220 ms) first: once USB is attached the
    // host expects answers within milliseconds, and a long pause here made it
    // give up on enumeration.
    let app = new_app(&mut clk);
    native::main_phase(0, clk.now_ms());
    run(clk, app)
}

/// The App lives in a static (`APP`), built by its own never-inlined
/// function: App::new's ~11 KiB of temporaries (the sub-structs before they
/// move into the App) are popped before `run`'s frame is pushed, instead of
/// sitting in main's frame under every main-loop call (stack headroom).
static mut APP: core::mem::MaybeUninit<app::App> = core::mem::MaybeUninit::uninit();

#[inline(never)]
fn new_app(clk: &mut Clock) -> &'static mut app::App {
    // Called once (main).
    unsafe { (*core::ptr::addr_of_mut!(APP)).write(app::App::new(clk)) }
}

/// The rest of the start-up and the main loop (main's frame stays small).
#[inline(never)]
fn run(mut clk: Clock, app: &'static mut app::App) -> ! {
    // The first frame, before the touch reset (~0.22 s) and before USB
    // attaches: a full draw (~20 ms) then cannot delay the enumeration, so
    // the loop below draws from its first pass (no wait for the host, nor
    // 3 s without one).
    if !cfg!(feature = "keep-screen") {
        lcd::fill_rect(0, 0, lcd::W, lcd::H, ui::BG);
    }
    app.tick(&mut clk, true);
    lcd::defer_flush();
    native::main_phase(1, clk.now_ms());
    // The generated USB volume's tables and files (vdisk.rs, ~0.47 s on the
    // device): after the first frame, before USB attaches.
    native::before_usb();
    let mut tp = touch::Touch::new(&mut clk);
    wdt::kick();
    native::main_phase(2, clk.now_ms());

    let hw = usb::init_hw(&mut clk);
    wdt::kick();
    fail(ST_PLLSAI, !hw.pllsai_locked);
    fail(ST_CORE, !hw.core_idle);

    let bus = synopsys_usb_otg::UsbBus::new(usb::UsbFs, unsafe {
        &mut *core::ptr::addr_of_mut!(EP_MEMORY)
    });
    // USBTMC (interface 0, SCPI incl. the DBG: debug commands) and USB mass
    // storage (interface 1) on the SPI flash. Two IN endpoints + EP0 of the
    // four of USBFS.
    let mut tmc = usbtmc::Usbtmc::new(&bus);
    // With `vdisk` (native) the one LUN is the read-only volume generated by
    // vdisk.rs (its tables built in native::before_usb, above); the SPI flash is
    // internal storage. Without it, LUN 0 = the SPI disk.
    let mut msc = msc::Msc::new(&bus, unsafe { &mut *core::ptr::addr_of_mut!(MSC_IO) }, unsafe { &mut *core::ptr::addr_of_mut!(MSC_BLK) }, 1);
    #[allow(unused_mut)]
    let mut disk = spiflash::SpiFlash::new();
    #[cfg(feature = "vdisk")]
    let mut info = vdisk::VDisk;
    // Refresh debounce of the generated volume: dlog::gen() at the last
    // change seen, and when.
    #[cfg(feature = "vdisk")]
    let (mut vd_gen, mut vd_t) = (dlog::gen(), 0u64);
    let mut dev = UsbDeviceBuilder::new(&bus, USB_VID_PID)
        .strings(&[StringDescriptors::default()
            .manufacturer("opendm40")
            .product("DM40 multimeter (opendm40)")
            .serial_number(uid::serial())])
        .unwrap()
        .build();
    // build() enabled the core; now force the GD32 power/VBUS bits.
    usb::force_power_no_vbus();

    let mut deadline = clk.now_ms() + INITIAL_HOLD_MS;
    let mut feeding = true;
    let mut reset_at: Option<u64> = None;

    // One SCPI session per transport (reply buffer, error queue, pending
    // READ?/MEAS? wait): [USB] = USBTMC, [BT] = Bluetooth serial (bt.rs;
    // untrusted, DBG:* refused). Replies go out through bt::ScpiPort.
    let mut scpis = [scpi::Scpi::new(), scpi::Scpi::new()];
    // The message each transport is running, command by command, and its
    // joined reply (scpi_msg.rs: compound messages, IEEE 488.2 order).
    let mut msgs = [scpi_msg::Msg::new(), scpi_msg::Msg::new()];
    // SCPI READ?/MEASure?/FETCh? waiting for a DMM reading: (min seq, settle,
    // start ms, expiry ms). The reply is the first reading with seq >= min,
    // at whatever averaging it has (READ?: the next conversion, ~0.1-0.4 s).
    // settle (MEASure:<f>? that changed the function): also wait for a full
    // averaging window, but at most SCPI_SETTLE_MS after the command.
    // Expiry: with a reading of the current function already there (READ? /
    // MEAS? without a function change) SCPI_QUICK_MS after the command, then
    // that latest reading answers (sigrok's 1 s USBTMC timeout). Without one
    // (just switched: MEAS:<f>?, CONF then READ?) SCPI_WAIT_MS after the
    // settle deadline, then +9.9E37 and "Data stale". A reply is never sent
    // after its query was given up (abort / clear / a new command): a late
    // reply would shift every later one.
    // A FETCh? before any reading starts its wait SCPI_QUICK_MS before
    // its expiry (SCPI_WAIT_MS after the deadline it was asked at), so the
    // ohm hunt extension below (waits longer than that) leaves it alone.
    // Per transport ([USB], [BT]); a BT line is taken only while [BT] is None.
    let mut scpi_wait: [Option<(u32, bool, u64, u64)>; 2] = [None, None];
    // Who made the DMM's last function change, for the READ? cap: func_key =
    // (function as FUNCtion? names it, temperature sensor) as last seen
    // (`scpi_func_key`: an autorange step V <-> mV or between the A shunts
    // is no change); host_key[src] = that key
    // when transport src's SCPI made the change (CONF, MEAS:<f>?, FUNC,
    // TEMP:TRAN), None after any other change (touch / keys / phone app /
    // DBG:TAP / the other transport). Only then may READ? / MEAS? wait for
    // the settle; otherwise SCPI_QUICK_MS at most (scpi.rs header). A wait
    // that such a change overtakes answers at once (WAIT_ORPHAN).
    let mut func_key = scpi_func_key(&app.dmm);
    let mut host_key: [Option<(u8, u8)>; 2] = [None, None];
    // Data logger (FR-38): owns the HY3131 AD1 fast stream while active.
    let logger = unsafe { &mut *core::ptr::addr_of_mut!(LOGGER) };
    logger.set_osr(logger::DEFAULT_OSR);
    let mut logging_prev = false;
    // DBG:AD2:DRAW (src/ad2exp.rs): 0 no logger panel, 1 panel (default),
    // 2 panel + a full DMM-view redraw every 100 ms; next forced redraw, ms.
    let mut log_draw = 1u8;
    let mut log_redraw_at = 0u64;
    // Fast logger to the disk (fastlog.rs): LOG:DEST DISK, LOG:DURation
    // (s, 0 = until stopped / full), a session was running last pass, the
    // tap-to-stop state (a release seen since the session began).
    let mut log_disk = false;
    let mut log_dur_s = 60u32;
    let mut fast_prev = false;
    let mut fast_tap_armed = false;
    // The Wave view's stream paused for a disk job (below).
    let mut wave_paused = false;
    let mut bt = bt::Bt::new();
    let mut gestures = gesture::Tracker::new();
    let mut touch_events = 0u32;
    let mut next_touch = 0u64;
    let mut next_poll = 0u64;
    let mut draw_hold_until = 0u64;
    // DBG:KEY n,ms: the deferred key-up (code, due time).
    let mut dbg_key_up: Option<(u8, u64)> = None;

    // Loop timing (debug): longest iteration in the last second, us.
    let mut loop_t0 = unsafe { mmio::r32(0xE000_1004) };
    let mut loop_max = 0u32;
    // Last pass with host disk I/O (a READ data phase, a WRITE): the
    // screenshot preparation's sector erases wait ~0.5 s after it.
    #[cfg(feature = "vdisk")]
    let mut host_io_t = 0u64;
    #[cfg(feature = "dbg-diag")]
    let mut loop_max_shown = 0u32;
    let mut loop_win = 0u64;
    // Section profile (debug): max us per section in the last second:
    // 0 usb+scpi, 1 tick, 2 touch, 3 scope, 4 bt, 5 dmm poll+draw.
    let mut sec = [0u32; 6];
    #[cfg(feature = "dbg-diag")]
    let mut sec_shown = [0u32; 6];
    // Streamed USBTMC reply: last progress (usbtmc stream_pos) and its time.
    let mut stream_seen = (usize::MAX, 0u64);
    // A plain reply made no progress for STREAM_STALL_MS: no more polling
    // for it (until it ends).
    let mut reply_stalled = false;
    let cyc = || unsafe { mmio::r32(0xE000_1004) };

    loop {
        let now = clk.now_ms();
        // Profiler (DBG:PROF?, src/prof.rs): loop period + the `lap` chain.
        prof::pass(now);
        boot::mark_healthy(now);
        let t = unsafe { mmio::r32(0xE000_1004) };
        loop_max = loop_max.max(t.wrapping_sub(loop_t0) / 200);
        loop_t0 = t;
        if now >= loop_win {
            loop_win = now + 1000;
            #[cfg(feature = "dbg-diag")]
            {
                loop_max_shown = loop_max;
                sec_shown = sec;
            }
            loop_max = 0;
            sec = [0; 6];
        }
        let mut ts = cyc();
        let mut mark = |k: usize, ts: &mut u32| {
            let t = cyc();
            sec[k] = sec[k].max(t.wrapping_sub(*ts) / 200);
            *ts = t;
        };
        let fed = feeding && now < deadline;
        wdt::set_feeding(fed);
        if fed {
            wdt::kick();
        }
        if reset_at.map(|t| now >= t).unwrap_or(false) {
            app::finish_disk_writes(now); // never cut a FAT / dir write
            unsafe { mmio::w32(SCB_AIRCR, 0x05FA_0004) };
        }

        dev.poll(&mut [&mut tmc, &mut msc]);
        // M4: no host disk writes while the logger needs a constant interval.
        msc.set_write_inhibit(logger.disk_writes_blocked());
        msc.set_usb_state(dev.state());
        app.usb_host = dev.state() == UsbDeviceState::Configured;
        power::USB_CONF.store(app.usb_host, core::sync::atomic::Ordering::Relaxed);
        // The INFO/ mirror's sectors while the stream runs: at most one per
        // pass, paced against the fast logger (vdisk.rs `mirror_pass`).
        #[cfg(feature = "vdisk")]
        vdisk::mirror_pass(fastlog::mirror_level(logger.waiting(), logger::CAP as u32));
        #[cfg(feature = "vdisk")]
        msc.poll(&mut [&mut info]);
        #[cfg(not(feature = "vdisk"))]
        msc.poll(&mut [&mut disk]);
        // A host READ of the disk: poll on while its data phase runs, up to
        // a budget per pass (full-speed bulk: <= 19 packets of 64 bytes per
        // 1 ms frame, one in flight at a time; the host takes one every
        // ~50 us). Less while a logger session runs (the AD1 main-loop
        // sampler, the fast logger's drain).
        if msc.reading() {
            let budget = if logger.active() || fastlog::active() { MSC_PUMP_LOG_CYC } else { MSC_PUMP_CYC };
            let t0 = cyc();
            while msc.reading() && cyc().wrapping_sub(t0) < budget {
                dev.poll(&mut [&mut tmc, &mut msc]);
                #[cfg(feature = "vdisk")]
                msc.poll(&mut [&mut info]);
                #[cfg(not(feature = "vdisk"))]
                msc.poll(&mut [&mut disk]);
            }
            #[cfg(feature = "vdisk")]
            {
                host_io_t = now;
            }
        }
        #[cfg(feature = "vdisk")]
        if msc::HOST_WRITING.load(core::sync::atomic::Ordering::Relaxed) {
            host_io_t = now;
        }
        // A USB disk READ / WRITE: activity for the idle shutdown.
        if msc::HOST_IO.swap(false, core::sync::atomic::Ordering::Relaxed) {
            app.note_host_activity(now);
        }
        // A USBTMC reply going out: its packets go out as the host takes
        // them, polled like a disk READ above, within the same budget per
        // pass (one packet per pass otherwise: a 6 KB MMEM:DATA? piece took
        // ~95 passes, 1.2 .. 7 s on the scope page with CH2 and the decoder,
        // device #174). A streamed reply (WAV:DATA? of the scope record,
        // scope_wav.rs) also while it waits for the host's next request; the
        // scope keeps its record (no RUN) until the last packet left.
        if !tmc.sending() {
            reply_stalled = false;
        }
        let streaming = tmc.streaming();
        if streaming || (tmc.sending() && !reply_stalled) {
            let budget = if logger.active() || fastlog::active() { MSC_PUMP_LOG_CYC } else { MSC_PUMP_CYC };
            let t0 = cyc();
            while (tmc.streaming() || tmc.sending()) && cyc().wrapping_sub(t0) < budget {
                dev.poll(&mut [&mut tmc, &mut msc]);
            }
            // A host that stopped reading and sends nothing more would hold
            // the scope (and this budget) for good: give a stream up, stop
            // polling for a plain reply after STREAM_STALL_MS without
            // progress.
            let p = tmc.stream_pos();
            if p != stream_seen.0 {
                stream_seen = (p, now);
            } else if now >= stream_seen.1 + STREAM_STALL_MS {
                if streaming {
                    tmc.cancel_stream();
                } else {
                    reply_stalled = true;
                }
            }
        } else {
            stream_seen = (usize::MAX, now);
        }
        app.scope.host_read = tmc.streaming() && scope_wav::reads_record();
        // Constant-interval HY3131 sampling, paced by the DWT / AD1F flag and run
        // every iteration (cheap when inactive). Kept independent of the GUI loop.
        let was_wave = logger.wave();
        if logger.poll() {
            // The AD2 stream ISR lost the bus to a main-loop access (hy3131
            // `guard`; a function change is caught by the auto-stop below
            // first): the session ended; say so like the auto-stop (the
            // wave view: it closes with a note).
            if was_wave {
                app.wave_lost(&mut clk, "Wave stopped: DMM changed");
                app.inrush_lost(&mut clk, "Stopped: DMM changed");
            } else {
                for s in scpis.iter_mut() {
                    s.set_err("Settings conflict");
                }
            }
        }
        // The fast logger to the disk (fastlog.rs): Prep / Run / Stop slices;
        // it asks for the logger session to start / stop.
        {
            let lon = logger.host_active();
            // The inrush save (app_inrush.rs): a session whose records are
            // the captured trace, not the logger ring (no logger runs).
            let ev = if app.inrush_saving() {
                fastlog::poll(false, &mut |b: &mut [fastlog::Rec]| app.inrush_drain(b))
            } else {
                fastlog::poll(lon, &mut LogSrc(&mut *logger))
            };
            let fa = fastlog::active();
            // The rare part (a start / stop / end / Record page request)
            // in one cold call.
            if ev != fastlog::Ev::None || fa != fast_prev || app.fast_req_pending() {
                // No region C code while the stream ISR runs (a C fetch stalls
                // its entry; device e656df3: max_lat 12 us, 1 late sample at
                // 38 400 S/s): the session's logger stops here, in B, before
                // the cold call, and starts after it (`fast_events` prepares).
                if matches!(ev, fastlog::Ev::Stop | fastlog::Ev::Restart) && logger.host_active() {
                    logger.stop();
                }
                let (conflict, go) = fast_events(app, &mut clk, logger, ev, fa, fast_prev, now);
                if go {
                    logger.start(app.dmm.mode(), app.dmm.range());
                }
                if conflict {
                    for s in scpis.iter_mut() {
                        s.set_err("Settings conflict");
                    }
                }
                if ev == fastlog::Ev::Start {
                    fast_tap_armed = false;
                }
            }
            fast_prev = fa;
            fastlog::set_cfg(logger.osr(), logger.chop(), logger.req_hz());
        }
        bt.pump(); // BT TX DMA: next segment of a long reply
        prof::lap(prof::USB);
        tmc.expire_partial();
        if tmc.take_cleared() {
            // USBTMC INITIATE_CLEAR / bus reset: no reply to a pending query.
            scpi_wait[USB] = None;
            msgs[USB].reset();
        }
        if bt.take_left() {
            // The Bluetooth host left: its pending query is not answered to
            // the next host (#122).
            scpi_wait[BT] = None;
            msgs[BT].reset();
        }
        if scpi_func_key(&app.dmm) != func_key {
            // Changed on the meter (or by DBG:KEY's deferred key-up): no host
            // waits for its settle; a READ? already waiting answers now.
            func_key = scpi_func_key(&app.dmm);
            host_key = [None, None];
            for w in scpi_wait.iter_mut().flatten() {
                w.1 = false;
                w.3 = WAIT_ORPHAN;
            }
        }
        let mut msg = [0u8; 256];
        let mut msg_n: usize;
        let mut src = USB;
        // The rest of a message that waited for a reading runs first (its
        // reply is due before the next message's).
        let resume = if msgs[USB].ready() {
            Some(USB)
        } else if msgs[BT].ready() {
            Some(BT)
        } else {
            None
        };
        if let Some(s) = resume {
            src = s;
        } else if let Some(cmd) = tmc.take_command() {
            // H1: a host session counts as activity for idle dim / power-off.
            app.note_host_activity(now);
            msg_n = cmd.len().min(256);
            msg[..msg_n].copy_from_slice(&cmd[..msg_n]);
            // Update-file upload data: raw binary, taken before the SCPI
            // parser (upload.rs; USB only: BT lines never get here).
            if let Some(m) = msg[..msg_n].strip_prefix(b"DBG:UPLOAD:DATA ") {
                if let Err(e) = upload::data_msg(m, now) {
                    scpis[USB].set_err(e);
                }
                msg_n = 0;
            }
            // A new message while a READ?/MEAS? is still waiting: the host
            // gave up on it (IEEE 488.2 query interrupted); no late reply.
            if scpi_wait[USB].take().is_some() {
                scpis[USB].set_err("Query INTERRUPTED");
            }
            if msg_n > 0 {
                msgs[USB].start(&msg[..msg_n]);
            } else {
                msgs[USB].reset();
            }
            // Any host traffic keeps the debug-build watchdog fed.
            deadline = deadline.max(now + KEEPALIVE_MS);
        } else if scpi_wait[BT].is_none() && !msgs[BT].active() {
            // SCPI over Bluetooth: one line at a time, the next only after
            // the previous query was answered (serial-instrument order).
            // DBG:* lines come back as Err (refused, nothing runs).
            match bt.take_line() {
                Some(Ok(l)) if app.bt_on => {
                    app.note_host_activity(now);
                    msg_n = l.len().min(256);
                    msg[..msg_n].copy_from_slice(&l[..msg_n]);
                    src = BT;
                    msgs[BT].start(&msg[..msg_n]);
                }
                Some(Err(e)) if app.bt_on => scpis[BT].set_err(e),
                _ => {}
            }
        }
        // One command of the message per pass of this loop (scpi_msg.rs);
        // a READ? / MEAS? / FETC? that waits pauses it until its reading.
        while let Some(msg_n) = msgs[src].next(&mut msg) {
            #[cfg(feature = "dbg-diag")]
            let dbg_ctl = tmc.dbg_ctl;
            #[cfg(feature = "dbg-diag")]
            let bt_tx = (bt.tx_dma_ok, bt.tx_dropped);
            let (mut scpi, port): (&mut scpi::Scpi, &mut dyn bt::ScpiPort) =
                if src == BT { (&mut scpis[BT], &mut bt) } else { (&mut scpis[USB], &mut tmc) };
            // Replies sent from the port's own buffer (block data) are seen
            // here: they are the message's response.
            let mut sp = SentPort { inner: port, sent: false };
            let port: &mut dyn bt::ScpiPort = &mut sp;
            // dmm.func(): the stock mode, 16 / 17 = V / A AC+DC (stable while
            // AC+DC alternates the HY3131 mode).
            let mut out = scpi.handle(&msg[..msg_n], app.dmm.func());
            let wav_block = matches!(out, scpi::Out::Instr(scpi::Cmd::WavData)) && scope_wav::is_block(app);
            if msgs[src].multi() && (block_reply(&out) || wav_block) {
                // Block data cannot be joined with other replies.
                scpi.len = 0;
                scpi.set_err("Query error");
                out = scpi::Out::Text;
            }
            // READ? / FETCh? right after a FUNCtion? / CONFigure? that named
            // another function (changed on the meter in between): no reading
            // to file under the function the host was told (scpi::take_asked).
            if matches!(out, scpi::Out::Measure(None) | scpi::Out::Fetch) && stale_read(scpi, app.dmm.func()) {
                out = scpi::Out::Text;
            }
            // DBG:TAP / KEY / DRAG stand in for the user: a change they make
            // is not the host's (READ? cap).
            let by_host = !matches!(out, scpi::Out::Dbg(_));
            let reply = match out {
                scpi::Out::None => false,
                scpi::Out::Text => true,
                scpi::Out::Configure(m) => {
                    app.select_func(&mut clk, m); // no-op when already selected
                    false
                }
                scpi::Out::Measure(m) => {
                    if logger.active() {
                        // The DMM poll is suspended while logging owns the HY3131;
                        // do not arm scpi_wait or the reply would never come.
                        scpi.len = 0;
                        scpi.put_bytes(b"+9.90000000E+37");
                        true
                    } else {
                        // The multimeter needs its analog side powered (a
                        // power-up starts a settle: the host's doing).
                        let d0 = app.dmm.deadline();
                        app.dmm_power(&mut clk);
                        let changed = m.is_some_and(|m| !app.is_func(m));
                        if let Some(m) = m.filter(|_| changed) {
                            app.select_func(&mut clk, m);
                        }
                        // The settle this query may wait for was requested by
                        // this host: this message (MEAS:<f>? changing the
                        // function), its earlier CONF / FUNC / TEMP:TRAN, or
                        // the power-up. Not after a change made on the meter:
                        // READ? then answers within SCPI_QUICK_MS (latest
                        // reading, else +9.9E37 and "Data stale"), as libsigrok
                        // times out after 1 s.
                        let host_settle = changed
                            || app.dmm.deadline() != d0
                            || host_key[src] == Some(scpi_func_key(&app.dmm));
                        // meter.live() is reset on every function change.
                        // *IDN? emulation (scpi::idn_emul): the host does not
                        // wait after CONF, never hold a reply past its 1 s.
                        // A rail wake (the DMM rail was off: Scope / Gen
                        // page) waits for its first reading, not the stale one.
                        let fresh = app.meter.live().is_some() && !app.dmm.waking();
                        let expire = if fresh || scpi::idn_emul() != 0 || !host_settle {
                            now + SCPI_QUICK_MS
                        } else if app.dmm.cpl_auto() {
                            (app.dmm.entry_ms() + AUTO_WAIT_MS).max(now + SCPI_QUICK_MS)
                        } else if app.dmm.hy_mode() == 14 && !app.dmm.pw.on {
                            // Hz: the no-signal decision (~3.3 s after the
                            // entry) or a slow first gate (below ~1 Hz), as
                            // long as libsigrok allows (AUTO_WAIT_MS).
                            (app.dmm.entry_ms() + AUTO_WAIT_MS).max(now.max(app.dmm.deadline()) + SCPI_WAIT_MS)
                        } else {
                            now.max(app.dmm.deadline()) + SCPI_WAIT_MS
                        };
                        let min_seq = app.last_raw.map(|r| r.seq.wrapping_add(1)).unwrap_or(0);
                        scpi_wait[src] = Some((min_seq, changed, now, expire));
                        false
                    }
                }
                // The data logger owns the HY3131: CONF selects the function
                // as before, a range / resolution change is refused.
                scpi::Out::Instr(scpi::Cmd::Conf(f, ..)) if logger.active() => {
                    app.select_func(&mut clk, f);
                    false
                }
                scpi::Out::Instr(scpi::Cmd::DbgSrc(Some(_))) if logger.active() => {
                    scpi.set_err("Settings conflict");
                    false
                }
                scpi::Out::Instr(scpi::Cmd::Range(_, op)) if logger.active() && !op.is_query() => {
                    scpi.set_err("Settings conflict");
                    false
                }
                // WAV:DATA? other than SCReen + BYTE: a block of up to
                // scope::CAP words, streamed over USBTMC (scope_wav.rs).
                scpi::Out::Instr(_) if wav_block => {
                    scpi.len = 0;
                    scope_wav::data(app, port, scpi);
                    false
                }
                scpi::Out::Instr(c) => {
                    scpi.len = 0;
                    app.scpi(&mut clk, c, &mut scpi)
                }
                scpi::Out::Fetch => match app.meter.shown().copied() {
                    // A range change with no reading yet (READ?'s rule; not
                    // while HOLD freezes the readout).
                    Some(r) if (app.dmm.reading_pending() || !app.dmm.reading_current(&r)) && app.meter.hold_state() == 0 => {
                        scpi.len = 0;
                        scpi.put_bytes(scpi::NAN);
                        true
                    }
                    Some(r) => {
                        scpi.len = 0;
                        scpi.put_reading(&r);
                        true
                    }
                    None => {
                        if logger.active() {
                            scpi.len = 0;
                            scpi.put_bytes(b"+9.90000000E+37");
                            true
                        } else if app.dmm.cpl_auto() || app.dmm.range_moving() {
                            // AUTO coupling before its first decision (its
                            // values are provisional, switch-latency.md 6):
                            // +9.91E37 at once, not a wait that ends in the
                            // provisional part (device 6bc494a: 1-3 s blocks).
                            // The same after a range step since the entry with
                            // no reading of the new range yet (READ?'s rule, as
                            // the Some arm), not a wait that a range hunt keeps
                            // moving (device 1a11eda: RES AUTO stepping 6 M <->
                            // 600 k, 8 of 8 FETCh? silent for the driver's 5 s).
                            scpi.len = 0;
                            scpi.put_bytes(scpi::NAN);
                            true
                        } else {
                            let expire = now.max(app.dmm.deadline()) + SCPI_WAIT_MS;
                            scpi_wait[src] = Some((0, false, expire - SCPI_QUICK_MS, expire));
                            false
                        }
                    }
                },
                scpi::Out::Dbg(_) if !port.trusted() => {
                    scpi.set_err(bt::proto::REFUSED); // backstop; take_line refuses first
                    false
                }
                scpi::Out::Dbg(d) => match d {
                    scpi::Dbg::Upload(op, a, b) => {
                        scpi.len = 0;
                        let r = match op {
                            0 => upload::begin(a, b, now).map(|_| None),
                            1 => upload::end().map(|(size, crc)| Some(([size, crc, 0], 2))),
                            2 => {
                                upload::abort();
                                Ok(None)
                            }
                            _ => {
                                let (on, got, size) = upload::status();
                                Ok(Some(([on as u32, got, size], 3)))
                            }
                        };
                        match r {
                            Ok(Some((v, n))) => {
                                for (i, x) in v[..n].iter().enumerate() {
                                    if i > 0 {
                                        scpi.put_bytes(b",");
                                    }
                                    scpi.put_u32(*x);
                                }
                                true
                            }
                            Ok(None) => false,
                            Err(e) => {
                                scpi.set_err(e);
                                false
                            }
                        }
                    }
                    scpi::Dbg::CapDisMs(k) => {
                        if let Some(k) = k {
                            crate::dmm::set_cap_dis_ms(k);
                            false
                        } else {
                            scpi.len = 0;
                            put_u32(&mut scpi, crate::dmm::cap_dis_ms());
                            true
                        }
                    }
                    scpi::Dbg::CapJumpN(k) => {
                        if let Some(k) = k {
                            crate::dmm::set_cap_jumpn(k);
                            false
                        } else {
                            scpi.len = 0;
                            put_u32(&mut scpi, crate::dmm::cap_jumpn());
                            true
                        }
                    }
                    scpi::Dbg::CapDisEnd(k) => {
                        if let Some(k) = k {
                            crate::dmm::set_cap_dis_end(k);
                            false
                        } else {
                            scpi.len = 0;
                            put_u32(&mut scpi, crate::dmm::cap_dis_end());
                            true
                        }
                    }
                    scpi::Dbg::CapDisMode(k) => {
                        if let Some(k) = k {
                            crate::dmm::set_cap_dismode(k);
                            false
                        } else {
                            scpi.len = 0;
                            put_u32(&mut scpi, crate::dmm::cap_dismode());
                            true
                        }
                    }
                    scpi::Dbg::CapDisRef(k) => {
                        if let Some(k) = k {
                            crate::dmm::set_cap_disref(k);
                            false
                        } else {
                            scpi.len = 0;
                            put_u32(&mut scpi, crate::dmm::cap_disref());
                            true
                        }
                    }
                    scpi::Dbg::CapIdis(k) => {
                        if let Some(k) = k {
                            crate::dmm::set_cap_idis(k);
                            false
                        } else {
                            scpi.len = 0;
                            put_u32(&mut scpi, crate::dmm::cap_idis());
                            true
                        }
                    }
                    scpi::Dbg::CapClass(k) => {
                        if let Some(k) = k {
                            crate::dmm::set_cap_class(k);
                            false
                        } else {
                            scpi.len = 0;
                            put_u32(&mut scpi, crate::dmm::cap_class() as u32);
                            true
                        }
                    }
                    scpi::Dbg::Cap => {
                        scpi.len = 0;
                        for (i, v) in app.dmm.cap_info().iter().enumerate() {
                            if i > 0 {
                                scpi.put_bytes(b",");
                            }
                            scpi.put_f32(*v);
                        }
                        true
                    }
                    scpi::Dbg::Fast => {
                        let (used, rej, vos, conv) = app.dmm.fast_info();
                        scpi.len = 0;
                        put_u32(&mut scpi, used);
                        scpi.put_bytes(b",");
                        put_u32(&mut scpi, rej);
                        scpi.put_bytes(b",");
                        if vos < 0.0 {
                            scpi.put_bytes(b"-");
                        }
                        put_u32(&mut scpi, (vos.abs() + 0.5) as u32);
                        scpi.put_bytes(b",");
                        put_u32(&mut scpi, (conv + 0.5) as u32);
                        true
                    }
                    scpi::Dbg::Idle(set) => {
                        app.dbg_idle(set, now, scpi);
                        set.is_none()
                    }
                    #[cfg(feature = "dbg-ohm6")]
                    scpi::Dbg::Ohm6(set) => {
                        app.dmm.ohm600_dbg(set, scpi);
                        set.is_none()
                    }
                    #[cfg(not(feature = "dbg-ohm6"))]
                    scpi::Dbg::Ohm6(_) => false,
                    scpi::Dbg::Ping(ms) => {
                        if ms > 0 {
                            deadline = deadline.max(now + (ms as u64).min(KEEPALIVE_MAX_MS));
                        }
                        let flags = hw.pllsai_locked as u32 | (hw.core_idle as u32) << 1;
                        let left = if feeding { scpi::keepalive_left(deadline, now) } else { 0 };
                        scpi.len = 0;
                        for (i, v) in [now as u32, flags, left].iter().enumerate() {
                            if i > 0 {
                                scpi.put_bytes(b",");
                            }
                            let mut t = [0u8; 10];
                            let mut i = t.len();
                            let mut v = *v;
                            loop {
                                i -= 1;
                                t[i] = b'0' + (v % 10) as u8;
                                v /= 10;
                                if v == 0 {
                                    break;
                                }
                            }
                            scpi.put_bytes(&t[i..]);
                        }
                        true
                    }
                    scpi::Dbg::Input(kind, x, y) => {
                        prof::touch(prof::cyc());
                        let p = touch::Point { x, y };
                        let g = match kind {
                            0 => Some(gesture::Gesture::Tap(p)),
                            1 => Some(gesture::Gesture::SwipeLeft),
                            2 => Some(gesture::Gesture::SwipeRight),
                            // DBG:KEY n[,ms]: a full click; y = hold ms
                            // (0: release now, else the key-up is deferred).
                            3 => {
                                if let Some((c, _)) = dbg_key_up.take() {
                                    app.gesture(&mut clk, gesture::Gesture::KeyUp(c)); // still held: release first
                                }
                                app.gesture(&mut clk, gesture::Gesture::KeyDown(x as u8));
                                if y == 0 {
                                    Some(gesture::Gesture::KeyUp(x as u8))
                                } else {
                                    dbg_key_up = Some((x as u8, now + y as u64));
                                    None
                                }
                            }
                            4 => gestures.synth_swipe_down(x as u8, now), // DBG:SWIPE D,n
                            _ => None,
                        };
                        if let Some(g) = g {
                            app.gesture(&mut clk, g);
                        }
                        false
                    }
                    scpi::Dbg::Drag(v) => {
                        let (p0, p1) = (touch::Point { x: v[0], y: v[1] }, touch::Point { x: v[2], y: v[3] });
                        gestures.synth_drag(p0, p1, v[4], v[5], v[6], now, |g| app.gesture(&mut clk, g));
                        // All events ran in this pass: time from the release
                        // to the first frame drawn (acquisition restart cost).
                        prof::touch(prof::cyc());
                        false
                    }
                    scpi::Dbg::Lcd(x, y, w, h, n) => {
                        // Raw GRAM words after RAMRD (dummy first) as #4 block.
                        let n = (n as usize).min((usbtmc::RSP_MAX - 8) / 2);
                        let mut hdr = [b'#', b'4', 0, 0, 0, 0];
                        let bytes = n * 2;
                        for (i, d) in [1000, 100, 10, 1].iter().enumerate() {
                            hdr[2 + i] = b'0' + ((bytes / d) % 10) as u8;
                        }
                        let buf = port.rsp_buf();
                        buf[..6].copy_from_slice(&hdr);
                        lcd::set_window_cmd(x, y, x + w - 1, y + h - 1, lcd::CMD_RAMRD);
                        for i in 0..n {
                            let v = lcd::read_data().to_le_bytes();
                            buf[6 + 2 * i] = v[0];
                            buf[7 + 2 * i] = v[1];
                        }
                        port.respond_len(6 + bytes);
                        draw_hold_until = now + 300;
                        false
                    }
                    // Never parsed without the features (scpi.rs).
                    #[cfg(not(feature = "dbg-touch"))]
                    scpi::Dbg::Touch(_) => false,
                    #[cfg(not(feature = "dbg-prof"))]
                    scpi::Dbg::Prof(_) => false,
                    #[cfg(feature = "dbg-touch")]
                    scpi::Dbg::Touch(clear) => {
                        if clear {
                            touch::log_clear();
                        } else {
                            let n = touch::log_text(port.rsp_buf(), now);
                            port.respond_len(n);
                        }
                        false
                    }
                    #[cfg(feature = "dbg-diag")]
                    scpi::Dbg::Diag => {
                        let mut d = touch_diag(&tp, touch_events, dbg_ctl, loop_max_shown, app.prof, sec_shown, bt_tx);
                        // 61: charge pins (bit 0 PB10, 1 PB11), bit 2 USB configured.
                        d[61] = app.power.pins() | (app.usb_host as u8) << 2;
                        scpi.len = 0;
                        scpi.put_bytes(b"#3");
                        let n = d.len();
                        scpi.put_bytes(&[b'0' + (n / 100) as u8, b'0' + (n / 10 % 10) as u8, b'0' + (n % 10) as u8]);
                        scpi.put_bytes(&d);
                        true
                    }
                    #[cfg(not(feature = "dbg-diag"))]
                    scpi::Dbg::Diag => false,
                    scpi::Dbg::Flash => {
                        scpi.len = 0;
                        let mut t = [0u8; 16];
                        for i in 0..6 {
                            t[i] = b"0123456789ABCDEF"[(disk.id >> (20 - 4 * i) & 0xF) as usize];
                        }
                        scpi.put_bytes(&t[..6]);
                        let (st, pp) = (screenshot::stats(), screenshot::prep_stats());
                        for v in [msc::BlockDevice::block_count(&disk), disk.failed() as u32, st[0], st[1], st[2], st[3], pp[0], pp[1], pp[2]] {
                            scpi.put_bytes(b",");
                            let s = dmm::format(&mut t, v as f32, 0);
                            scpi.put_bytes(s.as_bytes());
                        }
                        true
                    }
                    #[cfg(feature = "dbg-prof")]
                    scpi::Dbg::Prof(reset) => {
                        if reset {
                            prof::reset();
                        } else {
                            let n = prof::report(port.rsp_buf());
                            port.respond_len(n);
                        }
                        false
                    }
                    scpi::Dbg::Release => {
                        feeding = false;
                        false
                    }
                    scpi::Dbg::Reset => {
                        reset_at = Some(now + 50);
                        false
                    }
                    #[cfg(feature = "dbg-hyspi")]
                    scpi::Dbg::HySpi(n, div) => {
                        // Bus self-test (hyspi.rs): the DMM is paused and its
                        // row rewritten afterwards; the logger owns the chip.
                        if logger.active() {
                            scpi.set_err("Settings conflict");
                            false
                        } else {
                            app.dmm_power(&mut clk);
                            let len = hyspi::self_test(n as u32, div, port.rsp_buf());
                            port.respond_len(len);
                            app.dmm.start(&mut clk);
                            app.meter.reset();
                            app.last_raw = None;
                            false
                        }
                    }
                    #[cfg(feature = "dbg-hyspi")]
                    scpi::Dbg::HySpiWr(mode, div) => {
                        if logger.active() {
                            scpi.set_err("Settings conflict");
                            false
                        } else {
                            app.dmm_power(&mut clk);
                            let len = hyspi::write_test(mode, div, port.rsp_buf());
                            port.respond_len(len);
                            app.dmm.start(&mut clk);
                            app.meter.reset();
                            app.last_raw = None;
                            false
                        }
                    }
                    // DBG:HYSPI needs --features dbg-hyspi (scpi.rs refuses it).
                    #[cfg(not(feature = "dbg-hyspi"))]
                    scpi::Dbg::HySpi(..) | scpi::Dbg::HySpiWr(..) => false,
                    scpi::Dbg::Isr => {
                        let i = logger.isr_stats();
                        let cyc_ns = |c: u32| (c as u64 * 1_000_000_000 / clock::HCLK_HZ as u64) as u32;
                        #[cfg(feature = "ad2-isr")]
                        let (l, div) = (
                            logger::isr::lock_stats(),
                            hy3131::stream::div_used() as u32 + 1000 * hy3131::stream::cpha_used() as u32,
                        );
                        #[cfg(not(feature = "ad2-isr"))]
                        let (l, div) = ([0u32; 11], 0u32);
                        let vals = [
                            cfg!(feature = "ad2-isr") as u32,
                            logger.on_isr() as u32,
                            i[0],
                            if logger.on_isr() || i[0] != 0 { logger.captured() } else { 0 },
                            i[1],
                            i[2],
                            logger.overwritten(),
                            logger.bad(),
                            hy3131::timeouts(),
                            i[3],
                            cyc_ns(i[5]),
                            i[6] * 20, // timer counts at 50 MHz
                            l[4],
                            hy3131::forced_stops(),
                            l[3],
                            l[5], // signed
                            l[6],
                            l[2],
                            l[1],
                            div,
                            l[9],
                        ];
                        scpi.len = 0;
                        for (k, v) in vals.iter().enumerate() {
                            if k > 0 {
                                scpi.put_bytes(b",");
                            }
                            if k == 15 && (*v as i32) < 0 {
                                scpi.put_bytes(b"-");
                                put_u32(&mut scpi, (*v as i32).unsigned_abs());
                            } else {
                                put_u32(&mut scpi, *v);
                            }
                        }
                        scpi.put_bytes(b",0x");
                        for k in (0..8).rev() {
                            scpi.put_bytes(&[b"0123456789abcdef"[(l[7] >> (4 * k)) as usize & 15]]);
                        }
                        scpi.put_bytes(b",");
                        put_u32(&mut scpi, l[10]);
                        true
                    }
                    scpi::Dbg::Ad2(4, 255, _, _) => {
                        #[cfg(feature = "ad2-isr")]
                        let on = hy3131::stream::use_dma();
                        #[cfg(not(feature = "ad2-isr"))]
                        let on = false;
                        scpi.len = 0;
                        scpi.put_bytes(if on { b"1" } else { b"0" });
                        true
                    }
                    scpi::Dbg::Ad2(4, m, _, _) => {
                        #[cfg(feature = "ad2-isr")]
                        hy3131::stream::set_dma_req(m);
                        let _ = m;
                        false
                    }
                    scpi::Dbg::Ad2(3, div, _, cpha) => {
                        #[cfg(feature = "ad2-isr")]
                        hy3131::stream::set_div_req(div, cpha != 0);
                        let _ = (div, cpha);
                        false
                    }
                    scpi::Dbg::Ad2(2, level, _, _) => {
                        log_draw = level;
                        false
                    }
                    scpi::Dbg::Ad2(6, state, n, item) => {
                        dbg_n7(app, &mut clk, logger, &mut scpi, port, item, state, n);
                        false
                    }
                    scpi::Dbg::Ad2(5, r26, nr, flags) => {
                        dbg_dcv(app, &mut clk, logger, &mut scpi, port, r26, nr, flags);
                        false
                    }
                    scpi::Dbg::Ad2(kind, r26, n, b) => {
                        // Synchronous AD2 experiment in the current stock AC
                        // row; the row is rewritten afterwards.
                        if logger.active() || !matches!(app.dmm.mode(), 0 | 2 | 4..=6) {
                            scpi.set_err("Settings conflict");
                            false
                        } else {
                            app.dmm_power(&mut clk);
                            let buf = port.rsp_buf();
                            let len = if kind == 0 {
                                ad2exp::torn(r26, n as u32, b != 0, buf)
                            } else {
                                ad2exp::noise(r26, n as u32, b, buf)
                            };
                            port.respond_len(len);
                            app.dmm.start(&mut clk);
                            app.meter.reset();
                            app.last_raw = None;
                            false
                        }
                    }
                },
                scpi::Out::Inr(scpi::Inr::Data(start, count)) => {
                    port.respond_with(&mut |b| Some(app.inrush_data(start, count, b)));
                    false
                }
                scpi::Out::Inr(c) => app.inrush_query(&mut clk, c, &mut scpi),
                scpi::Out::Mmem(scpi::Mmem::Cat) => {
                    let mut err = None;
                    port.respond_with(&mut |b| csvlog::catalog_scpi(b).map_err(|e| err = Some(e)).ok());
                    if let Some(e) = err {
                        scpi.set_err(if e == csvlog::GErr::Busy { "Mass storage busy" } else { "Mass storage error" });
                    }
                    false
                }
                scpi::Out::Mmem(scpi::Mmem::Data(name, off, len)) => {
                    // #4nnnn + up to 6000 bytes (dlog.rs: a LOGnnn.CSV is
                    // generated from its LOGnnn.DLG).
                    let mut err = None;
                    port.respond_with(&mut |b| {
                        let len = (len as usize).min(b.len().saturating_sub(6));
                        match dlog::data(&name, off, &mut b[6..6 + len]) {
                            Ok(n) => {
                                b[..2].copy_from_slice(b"#4");
                                for (i, d) in [1000usize, 100, 10, 1].iter().enumerate() {
                                    b[2 + i] = b'0' + (n / d % 10) as u8;
                                }
                                Some(6 + n)
                            }
                            Err(e) => {
                                err = Some(e);
                                None
                            }
                        }
                    });
                    match err {
                        Some(dlog::DataErr::NotFound) => scpi.set_err("File name not found"),
                        Some(_) => scpi.set_err("Mass storage error"),
                        None => {}
                    }
                    false
                }
                // Deleting files over an open radio link: USB only.
                scpi::Out::Mmem(scpi::Mmem::Del(_)) if !port.trusted() => {
                    scpi.set_err(bt::proto::REFUSED);
                    false
                }
                scpi::Out::Mmem(scpi::Mmem::Del(name)) => {
                    match csvlog::delete(&name) {
                        Ok(()) => {}
                        Err(csvlog::GErr::NotFound) => scpi.set_err("File name not found"),
                        Err(csvlog::GErr::Busy) => scpi.set_err("Mass storage busy"),
                        Err(_) => scpi.set_err("Mass storage error"),
                    }
                    false
                }
                // Starting a CSV recording over an open radio link: USB only
                // (it blocks host writes, e.g. staging an update, until
                // stopped; review 6595d46 L5). LOG:CSV OFF / LOG:CSV? stay.
                scpi::Out::Log(scpi::Log::Csv(Some(true))) if !port.trusted() => {
                    scpi.set_err(bt::proto::REFUSED);
                    false
                }
                scpi::Out::Log(l) => match l {
                    scpi::Log::Csv(Some(on)) => {
                        if on {
                            if let Some(e) = app.rec_conflict() {
                                // The Wave view / inrush page stream: the
                                // DMM poll is paused, no rows would come.
                                scpi.set_err(e);
                            } else {
                                // The DMM needs its analog side (as MEAS?).
                                app.dmm_power(&mut clk);
                                if !app.rec_start(now) {
                                    scpi.set_err("Settings conflict");
                                }
                            }
                        } else {
                            csvlog::stop();
                        }
                        false
                    }
                    scpi::Log::Csv(None) => {
                        scpi.len = csvlog::status_scpi(&mut scpi.buf);
                        true
                    }
                    // Record page settings (decision 2026-10-22 #1).
                    l @ (scpi::Log::CsvInt(_) | scpi::Log::CsvDur(_) | scpi::Log::CsvChg(_)) => {
                        let r = rec_scpi(&mut scpi, l);
                        if !r {
                            app.invalidate();
                        }
                        r
                    }
                    scpi::Log::CsvFree => match csvlog::free_bytes() {
                        Ok(f) => {
                            scpi.len = 0;
                            put_u32(&mut scpi, f);
                            true
                        }
                        Err(e) => {
                            scpi.set_err(if e == csvlog::GErr::Busy { "Mass storage busy" } else { "Mass storage error" });
                            false
                        }
                    },
                    scpi::Log::State(Some(on)) => {
                        if let Some(e) = on.then(|| log_on_check(app, logger, log_disk, log_dur_s, port.trusted())).flatten() {
                            // Refused, or a disk session began (Err("")).
                            if !e.is_empty() {
                                scpi.set_err(e);
                            }
                        } else if on {
                            // AD1 needs a DC mode (1 DCV, 3 DCmV, 7/8/9 DCA),
                            // AD2 a stock AC row (0 ACV, 2 ACmV, 4/5/6 ACA) or
                            // DC V / mV (their AD2 rows, E7); never AC+DC or
                            // AUTO coupling (16/17/19/20: alternating rows,
                            // whose current phase is a plain mode), the power
                            // meter, ohm, cap, freq, temp (logger::Logger::mode_ok).
                            let m = app.dmm.mode();
                            let alt = matches!(app.dmm.func(), 16 | 17 | dmm::FUNC_V_AUTO | dmm::FUNC_A_AUTO) || app.dmm.pw.on;
                            if !alt && logger::Logger::mode_ok(logger.ad2(), m) {
                                app.dmm_power(&mut clk);
                                // Guarantee the full row is applied before the
                                // logger overrides R22 (AD1OSR) / R26 (AD2).
                                app.dmm.start(&mut clk);
                                logger.start(m, app.dmm.range());
                            } else {
                                scpi.set_err("Settings conflict");
                            }
                        } else if fastlog::active() {
                            fastlog::request_stop();
                        } else if !logger.wave() {
                            logger.stop();
                        }
                        false
                    }
                    scpi::Log::State(None) => {
                        scpi.len = 0;
                        scpi.put_bytes(if logger.host_active() || fastlog::active() { b"1" } else { b"0" });
                        true
                    }
                    // LOG:DISK? is polled by a host during a disk session
                    // (the AD2 ISR streaming): region B, not fast_scpi.
                    scpi::Log::Disk => {
                        fast_status(scpi);
                        true
                    }
                    scpi::Log::Dest(_) | scpi::Log::Dur(_) => {
                        fast_scpi(scpi, l, &mut log_disk, &mut log_dur_s, logger.host_active())
                    }
                    scpi::Log::Source(Some(ad2)) => {
                        // The source also selects what poll() reads: not
                        // switched under a running session.
                        if (logger.host_active() || fastlog::active()) && ad2 != logger.ad2() {
                            scpi.set_err("Settings conflict");
                        } else {
                            logger.set_ad2(ad2);
                        }
                        false
                    }
                    scpi::Log::Source(None) => {
                        scpi.len = 0;
                        scpi.put_bytes(if logger.ad2() { b"AD2" } else { b"AD1" });
                        true
                    }
                    scpi::Log::SRate(Some(hz)) => {
                        logger.set_req_hz(hz);
                        false
                    }
                    scpi::Log::SRate(None) => {
                        scpi.len = 0;
                        put_u32(&mut scpi, logger.req_hz());
                        true
                    }
                    scpi::Log::Gaps => {
                        scpi.len = 0;
                        let g = logger.gaps();
                        let vals = [logger.bad(), logger.max_gap_us(), g[0], g[1], g[2], g[3], g[4]];
                        for (i, v) in vals.iter().enumerate() {
                            if i > 0 {
                                scpi.put_bytes(b",");
                            }
                            put_u32(&mut scpi, *v);
                        }
                        true
                    }
                    scpi::Log::Osr(Some(n)) => {
                        logger.set_osr(n);
                        false
                    }
                    scpi::Log::Osr(None) => {
                        scpi.len = 0;
                        put_u32(&mut scpi, logger.osr() as u32);
                        true
                    }
                    scpi::Log::Chop(Some(on)) => {
                        logger.set_chop(on);
                        false
                    }
                    scpi::Log::Chop(None) => {
                        scpi.len = 0;
                        scpi.put_bytes(if logger.chop() { b"1" } else { b"0" });
                        true
                    }
                    scpi::Log::Rate => {
                        scpi.len = 0;
                        put_u32(&mut scpi, logger.measured_hz());
                        true
                    }
                    scpi::Log::Count => {
                        scpi.len = 0;
                        for (i, v) in [logger.buffered(), logger.captured(), logger.overwritten(), logger.missed(), logger.lost()]
                            .iter()
                            .enumerate()
                        {
                            if i > 0 {
                                scpi.put_bytes(b",");
                            }
                            put_u32(&mut scpi, *v);
                        }
                        true
                    }
                    scpi::Log::Scale => {
                        log_scale(&mut scpi, logger, app.dmm.ad2_scale());
                        true
                    }
                    scpi::Log::Preamble => {
                        scpi.len = 0;
                        let ad2 = logger.ad2();
                        let r26 = logger.r26();
                        let vals = [
                            logger.mode() as u32,
                            logger.range() as u32,
                            if ad2 { (r26 & 7) as u32 } else { logger.osr() as u32 },
                            logger.nominal_us(),
                            logger.measured_hz(),
                            logger.chop() as u32,
                            ad2 as u32,
                            if ad2 { logger.req_hz() } else { 0 },
                            logger.nominal_mhz(),
                            if ad2 { r26 as u32 } else { 0 },
                            (ad2 && logger.mode() == 0) as u32, // AC V: AC-coupled
                        ];
                        for (i, v) in vals.iter().enumerate() {
                            if i > 0 {
                                scpi.put_bytes(b",");
                            }
                            put_u32(&mut scpi, *v);
                        }
                        true
                    }
                    scpi::Log::Data => {
                        // Definite-length #4nnnn block straight into the USBTMC
                        // response buffer (6144 B), like Dbg::Lcd. scpi.buf is
                        // only 512 B and #3 caps at 999 bytes, so neither fits.
                        if fastlog::owns_ring() {
                            // The disk session drains the ring.
                            port.respond(b"#40000");
                        } else if !port.trusted() && logger.host_active() && (logger.ad2() || logger.nominal_mhz() > BT_MAX_HZ * 1000) {
                            port.respond(b"#40000");
                            scpi.set_err("Rate too high for Bluetooth");
                        } else {
                            let par = fmt_par(logger, app.dmm.ad2_scale());
                            port.respond_with(&mut |buf| Some(logger.encode_fmt(buf, &par)));
                        }
                        false
                    }
                    scpi::Log::Format(Some(f)) => {
                        logger.set_fmt(f);
                        false
                    }
                    scpi::Log::Format(None) => {
                        scpi.len = 0;
                        scpi.put_bytes(match logger.fmt() {
                            logger::FMT_PACKED => b"PACK",
                            logger::FMT_DLG => b"DLG",
                            _ => b"ROWS",
                        });
                        true
                    }
                },
            };
            if sp.sent {
                msgs[src].raw_sent();
            }
            if reply && !msgs[src].reply(&scpi.buf[..scpi.len]) {
                scpi.set_err("Out of memory");
            }
            if scpi_wait[src].is_some() {
                msgs[src].pause();
            }
            let key = scpi_func_key(&app.dmm);
            if key != func_key {
                func_key = key;
                host_key = [None, None];
                if by_host {
                    host_key[src] = Some(key);
                    scpi::forget_asked(&mut scpis[src]);
                }
                // A wait of the other transport (or any, after DBG:TAP / KEY)
                // was not for this change: it answers now (WAIT_ORPHAN).
                for (i, w) in scpi_wait.iter_mut().enumerate() {
                    if let Some(w) = w.as_mut().filter(|_| !by_host || i != src) {
                        w.1 = false;
                        w.3 = WAIT_ORPHAN;
                    }
                }
            }
            prof::lap(prof::SCPI);
            if scpi_wait[src].is_some() {
                break;
            }
        }
        if let Some(r) = msgs[src].finish() {
            let port: &mut dyn bt::ScpiPort = if src == BT { &mut bt } else { &mut tmc };
            port.respond(r);
        }

        // The DMM CH2 of the MCX scope page (app_ch2.rs): its stream starts,
        // restarts at a timebase in another rate band and stops here, before
        // the wave view's / inrush page's requests (their streams take the
        // logger over: CH2's is stopped first, `ch2_want` is None in their
        // views). A disk job stops it (region C code every pass, as the
        // wave view's pause); it starts again after. A stream it did not
        // start (a host session, the wave view, inrush) is left alone.
        {
            let want = app.ch2_want().filter(|_| !screenshot::busy() && !logger.host_active() && !fastlog::active());
            let mine = app.ch2_live() && logger.active() && logger.wave();
            if app.ch2_live() && !mine {
                app.ch2_lost(now); // a DMM change, or taken over
            }
            match want {
                Some(fs) if (!mine || app.ch2_fs() != fs) && (mine || !logger.active()) => {
                    let fresh = !logger.active();
                    if mine {
                        logger.stop();
                    }
                    if fresh {
                        // The full row first, as the wave view's entry.
                        app.dmm.start(&mut clk);
                    }
                    if app.dmm.ad2_scale().is_some() && logger::Logger::mode_ok(true, app.dmm.mode()) {
                        csvlog::catalog_warm(); // MMEM:CAT? while CH2 streams: from the cache
                        logger.start_wave(app.dmm.mode(), app.dmm.range(), fs);
                        app.ch2_started(fs, logger.nominal_mhz(), logger.begin_cyc());
                    } else {
                        app.ch2_stopped();
                    }
                }
                None if mine => {
                    logger.stop();
                    app.ch2_stopped();
                }
                _ => {}
            }
        }
        // Any path that reconfigures the DMM (a host CONFigure, a touch/key
        // FUNC/RANGE change, a BT mode-byte) rewrites the 22-byte HY3131 config
        // and R22 behind the logger's back. Detect that and auto-stop so the
        // host sees STATe?=0 + a SYST:ERR? rather than silently corrupted
        // samples (FR-38 forbids silent loss). Captured data stays readable.
        // The DMM wave view (scope page, source DMM): its stream starts,
        // restarts (a timebase in another rate band, a range step: the row
        // is rewritten between stop and start, so the bus guard never
        // fires for it) and stops here, where the logger lives.
        // A disk job (Save CSV, a screenshot: screenshot.rs, region C) in the
        // Wave view pauses its stream until the job is done: the job runs
        // region C code every pass (~20 us ISR entry stalls each; device
        // d8e249d: Save CSV in the Wave view, max_lat 21.7 us, 15 overruns).
        // The screen is STOPped (Save CSV) or frozen (a shot) meanwhile.
        if app.scope.wave_on() && (logger.wave() || wave_paused) {
            if logger.active() && screenshot::busy() {
                logger.stop();
                wave_paused = true;
            } else if wave_paused && !screenshot::busy() {
                wave_paused = false;
                if !logger.active() && app.dmm.ad2_scale().is_some() && logger::Logger::mode_ok(true, app.dmm.mode()) {
                    csvlog::catalog_warm(); // the job changed the disk: MMEM:CAT? from a cache while streaming
                    logger.start_wave(app.dmm.mode(), app.dmm.range(), app.scope.wave_fs());
                    app.wave_live();
                }
            }
        } else {
            wave_paused = false;
        }
        if let Some(r) = app.take_wave_req() {
            if logger.host_active() {
                app.wave_lost(&mut clk, "Data logger running");
            } else {
                // A fresh entry (no stream yet): the full row first, as
                // LOG:STATe ON does (the DMM rail may just have come up, the
                // sentinel may have AD1 / the peak hold running).
                let fresh = !logger.active();
                if logger.active() {
                    logger.stop();
                }
                if fresh && r == app::WaveReq::Start {
                    app.dmm.start(&mut clk);
                }
                if let app::WaveReq::Range(item) = r {
                    app.dmm.set_range(&mut clk, Some(item));
                }
                if let app::WaveReq::Coupling(item) = r {
                    app.wave_coupling(&mut clk, item);
                }
                if matches!(r, app::WaveReq::Range(_) | app::WaveReq::Coupling(_)) {
                    // The new scale's full repaint before the stream starts
                    // again (app_wave.rs `wave_rescale`).
                    app.wave_rescale(now >= draw_hold_until && !screenshot::frozen());
                }
                if r != app::WaveReq::Stop {
                    if app.dmm.ad2_scale().is_some() && logger::Logger::mode_ok(true, app.dmm.mode()) {
                        // MMEM:CAT? while the view streams: from the cache
                        // (device c92b605: a scan in one pass at 76800
                        // dropped ~90 ms of the view's ring).
                        csvlog::catalog_warm();
                        logger.start_wave(app.dmm.mode(), app.dmm.range(), app.scope.wave_fs());
                        if hy3131::ad2_rate_mhz(logger.r26()) / 1000 != app.scope.wave_fs() {
                            // The rung was capped (76.8 kS/s with CPU frames,
                            // DBG:AD2:DMA 0, or a forced divider): the record's
                            // time base would be wrong.
                            logger.stop();
                            app.wave_lost(&mut clk, "Wave: rate needs DMA frames");
                        } else {
                            app.wave_live();
                        }
                    } else {
                        app.wave_lost(&mut clk, "Wave: no AD2 row here");
                    }
                }
            }
            if r == app::WaveReq::Stop {
                // A view change out of the wave view held for the stop.
                app.wave_exit_done(&mut clk);
            }
        }
        // The inrush capture (app_inrush_live.rs): the ring into the
        // capture; its stream starts on the page's arm and stops when the
        // capture is complete, cancelled or the page closed.
        app.inrush_poll();
        if let Some(r) = app.take_inrush_req() {
            if logger.host_active() {
                app.inrush_lost(&mut clk, "Stopped: data logger running");
            } else {
                let fresh = !logger.active();
                if logger.active() {
                    logger.stop();
                }
                if r == inrush::Req::Start {
                    if fresh {
                        app.dmm.start(&mut clk);
                    }
                    if app.dmm.ad2_scale().is_some() && logger::Logger::mode_ok(true, app.dmm.mode()) {
                        csvlog::catalog_warm(); // MMEM:CAT? while armed: from the cache
                        logger.start_wave(app.dmm.mode(), app.dmm.range(), inrush::RATE_HZ);
                        app.inrush_live(logger.nominal_mhz());
                    } else {
                        app.inrush_lost(&mut clk, "Stopped: no AD2 row");
                    }
                } else {
                    app.inrush_stopped(&mut clk);
                }
            }
        }
        if logger.active() && (app.dmm.mode() != logger.mode() || app.dmm.range() != logger.range()) {
            let wave = logger.wave();
            logger.stop();
            if wave {
                app.wave_lost(&mut clk, "Wave stopped: DMM changed");
                app.inrush_lost(&mut clk, "Stopped: DMM changed");
            } else {
                for s in scpis.iter_mut() {
                    s.set_err("Settings conflict");
                }
            }
        }

        // While logging, the HY3131 belongs to the logger and the GUI is frozen
        // (FR-38: minimal on-screen change, constant sample interval). On the
        // ON->OFF edge, hand the chip back to the DMM with a clean reconfigure.
        // `busy`: the logger owns the HY3131 (a host session or the wave
        // view: no DMM poll, no BT serve); `logging`: a host session, which
        // also freezes the GUI (the wave view is a live scope page).
        let busy = logger.active();
        // A disk session (fastlog.rs) freezes the GUI from its Prep to its
        // last FAT write; tap to stop (below).
        // (Not the inrush save: no logger runs, the page stays live.)
        let logging = logger.host_active() || (fastlog::active() && !app.inrush_saving());
        // A toast during a session goes into the logger panel (dialog.rs
        // `toast_rect`): nothing repaints the view around it until the end.
        dialog::log_panel_on(logging && log_draw >= 1);
        if logging_prev && !busy {
            app.dmm.start(&mut clk);
            app.meter.reset();
            app.last_raw = None;
            // Take the logger panel down: repaint the view.
            dialog::log_panel_reset();
            app.invalidate();
        }
        logging_prev = busy;
        app.logger_busy = busy;

        // No drawing right after a GRAM read-back, so a multi-request
        // screenshot does not tear (the first frame was drawn before USB
        // attached, above the loop).
        // A screenshot reading the panel (screenshot.rs) freezes it too.
        let can_draw = now >= draw_hold_until && !logging && !screenshot::frozen();
        // DBG:AD2:DRAW 2: the DMM page's redraw load during a log session
        // (loss-per-rate experiment): a full view repaint every 100 ms, then
        // the panel again on top.
        let force_draw = logging && log_draw == 2 && now >= log_redraw_at && !screenshot::frozen();
        if force_draw {
            log_redraw_at = now + 100;
            app.invalidate(); // drawn by app.tick below; the panel after it
        }
        // A panel part only while the ring has room for it (dialog.rs
        // `log_panel_room`: device 2f56ff9, the first draw's hole at 76.8k).
        let panel_room = dialog::log_panel_room(now, logging.then(|| logger.waiting()));
        if logging && log_draw >= 1 && now >= draw_hold_until && !screenshot::frozen() && panel_room && !dialog::log_panel_idle(now) {
            // The logger panel (dialog.rs; one part per pass, ~1 s refresh).
            dialog::log_panel(
                now,
                dialog::LogInfo {
                    hz: logger.measured_hz(),
                    n: logger.captured(),
                    nominal_hz: (logger.nominal_mhz() + 500) / 1000,
                    ad2: logger.ad2(),
                    lost: logger.lost(),
                    disk: fastlog::panel(),
                },
            );
        }
        mark(0, &mut ts);
        // CSV recording: one bounded slice of a commit (csvlog.rs).
        prof::lap_skip();
        csvlog::poll(now);
        upload::poll(now);
        // The next screenshot's clusters erased in idle time (screenshot.rs
        // "idle-time preparation"): not while a disk job keeps data in free
        // clusters (csvlog commit, fast-log session, upload / staging; a
        // shot is checked inside), a logger session runs (constant interval;
        // the AD2 stream ISR: the step is region C), or the host used the
        // disk in the last 0.5 s. Only with the generated volume: a host
        // that writes the SPI disk itself (no `vdisk`) could be filling a
        // free cluster.
        #[cfg(feature = "vdisk")]
        screenshot::prep_poll(
            now,
            !csvlog::writing()
                && !fastlog::active()
                && !upload::busy()
                && !logger.disk_writes_blocked()
                && !hy3131::streaming()
                && now.saturating_sub(host_io_t) >= 500,
        );
        // The generated USB volume (vdisk.rs): rebuilt when the SPI files
        // changed (a recording ended, a screenshot / scope record, a delete),
        // once nothing changed for 1 s and no on-device writer holds the disk.
        #[cfg(feature = "vdisk")]
        {
            // Host writes on the volume (vwrite.rs): a delete's SPI part, a
            // firmware file's check + commit; Yes in its dialog = DBG:UPDATE.
            vwrite::poll(now, logger.disk_writes_blocked() || fastlog::holds_disk());
            if vwrite::take_install() && reset_at.is_none() {
                if boot::enter_loader_update() {
                    reset_at = Some(now + 50);
                }
            }
            if dlog::gen() != vd_gen {
                vd_gen = dlog::gen();
                vd_t = now;
            } else if dlog::stale() && now.saturating_sub(vd_t) >= 1000 && !csvlog::writing() && !screenshot::busy() && !fastlog::holds_disk()
                // Not while the AD2 stream ISR owns the bus (the Wave view,
                // an armed inrush capture): the refresh is region C code
                // (vdisk refresh / ours / scan_spi, dlog's directory scan),
                // ~20 us ISR entry stalls (device d8e249d: Save CSV in the
                // Wave view, max_lat 21.7 us, 15 overruns). It runs once the
                // stream stops; only a host sees the volume.
                && !hy3131::streaming()
            {
                vdisk::refresh(csvlog::recording_name());
            }
        }
        prof::lap(prof::CSV);
        app.tick(&mut clk, can_draw || force_draw);
        lcd::defer_flush(); // a repaint's batch (App::repaint)
        // The settings' copy on the SPI disk (setfile.rs): a few seconds
        // after a change, never during an AD1 logger session.
        setfile::poll(now, logger.disk_writes_blocked());
        if force_draw {
            dialog::log_panel_reset(); // the repaint covered it: draw it again
        }
        prof::lap(prof::TICK);
        mark(1, &mut ts);
        for src in [USB, BT] {
            // Power meter: a wait for the host's own settle follows its range
            // hunt (each step restarts the P / V / I cycle, ~1.9 s).
            if let Some(w) = scpi_wait[src].as_mut().filter(|w| w.1 && app.dmm.pw.on) {
                w.3 = w.3.max(app.dmm.deadline() + SCPI_WAIT_MS).min(w.2 + 15_000);
            }
            // Ohm: a host's settle wait (CONF:RES then READ?, MEAS:RES?)
            // follows the range hunt (each step moves the deadline) and the
            // vet of its last range (dmm.rs OHM_VET_*: one conversion more).
            // Not a FETCh? (its expiry stays SCPI_WAIT_MS after its deadline).
            if let Some(w) = scpi_wait[src].as_mut().filter(|w| w.3 > w.2 + SCPI_QUICK_MS && app.dmm.hy_mode() == 10 && !app.dmm.pw.on) {
                w.3 = w.3.max(app.dmm.deadline() + SCPI_WAIT_MS).min(w.2 + 8_000);
            }
            if !scpi_wait[src].is_some_and(|w| now >= w.3) {
                continue;
            }
            // No fresh DMM reading in time (a slow capacitance / temperature
            // range, a range hunt, a stalled converter): answer now rather
            // than after the host's timeout, with the latest reading of the
            // current function if there is one.
            // A wait the function changed under (WAIT_ORPHAN) or no reading
            // of the current function: SCPI NaN, not the overload value, so
            // that a host (libsigrok's DM40 profile) can skip it. The same
            // while a range change has no reading yet (Dmm::reading_pending:
            // the AC+DC settle, an autorange step): the latest reading is
            // from before it.
            let orphan = scpi_wait[src].is_some_and(|w| w.3 == WAIT_ORPHAN);
            scpi_wait[src] = None;
            let scpi = &mut scpis[src];
            scpi.len = 0;
            // AUTO coupling still deciding (AUTO_WAIT_MS): the part it shows.
            let live = app.meter.live().copied().filter(|_| !orphan && !app.dmm.reading_pending());
            match live.or_else(|| app.auto_prov().filter(|_| !orphan)) {
                Some(r) => scpi.put_reading(&r),
                None => {
                    scpi.put_bytes(scpi::NAN);
                    scpi.set_err("Data stale");
                }
            }
            msgs[src].reply(&scpi.buf[..scpi.len]); // the rest of the message runs next pass
        }
        if let Some((c, _)) = dbg_key_up.filter(|k| now >= k.1) {
            dbg_key_up = None;
            app.gesture(&mut clk, gesture::Gesture::KeyUp(c));
        }

        // Suspend touch while logging: there is no logger UI (the screen is
        // frozen), the power key lives in app.tick, and the touch I2C read
        // every 10 ms is the largest periodic jitter source for the sampler.
        // EXTI-latched INT edge (touch.rs): read the fresh report now, not on
        // the tick.
        let touch_irq = tp.as_ref().map_or(false, |t| t.irq_pending());
        // A disk session (fastlog.rs) keeps a coarse touch read (10/s; the
        // AD2 stream ISR does not care, AD1 at 640/s has 1.56 ms per
        // conversion): any new press after a release stops / cancels it.
        let fast_touch = logging && fastlog::active();
        if (now >= next_touch || touch_irq) && (!logging || fast_touch) {
            next_touch = now + if fast_touch { 100 } else { 10 };
            let t_rd = prof::cyc();
            prof::enter(prof::TOUCH);
            let mut reported = false;
            let g = match tp.as_mut().and_then(|t| t.poll(now)) {
                Some(ev) if fast_touch => {
                    if ev.points == 0 {
                        fast_tap_armed = true;
                    } else if fast_tap_armed {
                        fastlog::request_stop();
                        fast_tap_armed = false;
                    }
                    None
                }
                None if fast_touch => None,
                Some(ev) => {
                    touch_events = touch_events.wrapping_add(1);
                    reported = true;
                    gestures.event(&ev, now)
                }
                None => gestures.tick(now),
            };
            prof::exit(prof::TOUCH);
            // The touch test page draws what this poll logged (frames,
            // debounced state; app_touchtest.rs, region C: never during a
            // session).
            if app.view == ui::View::TouchTest && !logging {
                app.tt_poll(&mut clk, can_draw);
            }
            if let Some(g) = g {
                if reported {
                    prof::touch(t_rd); // latency from the report read
                }
                prof::enter(prof::GEST);
                if app.gesture(&mut clk, g) {
                    if app.slider_active() { gestures.capture_direct() } else { gestures.capture() }
                }
                lcd::defer_flush();
                prof::exit(prof::GEST);
            }
        }
        mark(2, &mut ts);
        if !logging {
            prof::enter(prof::SCOPE);
            app.scope_poll(now, can_draw);
            lcd::defer_flush();
            prof::exit(prof::SCOPE);
        }
        mark(3, &mut ts);
        if now >= next_poll {
            next_poll = now + 5;
            prof::enter(prof::BT);
            // Module power (PC0) follows the BT setting.
            bt.power(app.bt_on);
            // The BT module AT init blocks; not while the logger samples.
            if app.bt_on && !busy {
                bt.init_module(&mut clk, app.dmm.model());
            }
            if let Some(f) = bt.poll() {
                if app.bt_on {
                    app.note_host_activity(now); // H1
                }
                // Don't let a BT mode-byte (cmd 06) reconfigure the HY3131 while
                // the logger owns it; the auto-stop guard above is the backstop.
                if app.bt_on && !busy {
                    let raw = app.last_raw;
                    if bt.serve(&f, &mut clk, &mut app.dmm, &mut app.meter, raw.as_ref(), &app.power, app.locked) {
                        // Phone app changed the function (cmd 06): the same
                        // path as a touch / SCPI change.
                        app.host_func_changed();
                    }
                }
            }
            prof::exit(prof::BT);
            mark(4, &mut ts);
            prof::enter(prof::DMM);
            if !busy {
                if let Some(raw) = app.dmm.poll(&mut clk) {
                    if raw.prov {
                        // A value a gate still holds: drawn dimmed only.
                        app.provisional(raw, can_draw);
                    } else if let Some(r) = app.reading(raw, can_draw) {
                        // A function change this pass (touch, phone app) is
                        // seen at the next loop top: no reading for a wait it
                        // overtook until then (WAIT_ORPHAN).
                        let same_func = scpi_func_key(&app.dmm) == func_key;
                        for src in [USB, BT] {
                        if let Some((_, settle, t0, _)) = scpi_wait[src].filter(|w| same_func && r.seq.wrapping_sub(w.0) < 0x8000_0000) {
                            // MEASure:<f>? after a function change: prefer a
                            // value at the full (+1 digit) resolution (the
                            // moving average holds AVG_FULL conversions; avg_n
                            // 0 = not an averaged range; overload never
                            // averages), but not past SCPI_SETTLE_MS.
                            let ready = !settle
                                || r.avg_n == 0
                                || r.avg_n >= dmm::AVG_FULL
                                || r.main.ol
                                || now >= t0 + SCPI_SETTLE_MS;
                            if ready {
                                scpi_wait[src] = None;
                                let scpi = &mut scpis[src];
                                scpi.len = 0;
                                scpi.put_reading(&r);
                                msgs[src].reply(&scpi.buf[..scpi.len]);
                            }
                        }
                        }
                    }
                }
            }
            // The beeper indicator follows the comparator read just now.
            app.beep_ind(now, can_draw, !busy);
            prof::exit(prof::DMM);
            mark(5, &mut ts);
        }
    }
}

// The MMEM:CAT? cache holds a whole reply (csvlog.rs `Cat`).
const _: () = assert!(csvlog::CAT_MAX == usbtmc::RSP_MAX);

/// A command whose reply is built in the transport's own buffer (block
/// data; not joinable with other replies of a message, scpi_msg.rs).
fn block_reply(o: &scpi::Out) -> bool {
    use scpi::{Dbg, Log, Mmem, Out};
    matches!(
        o,
        Out::Mmem(Mmem::Cat | Mmem::Data(..))
            | Out::Log(Log::Data)
            | Out::Inr(scpi::Inr::Data(..))
            | Out::Dbg(Dbg::Lcd(..) | Dbg::Touch(false) | Dbg::Prof(false) | Dbg::HySpi(..) | Dbg::HySpiWr(..))
            | Out::Dbg(Dbg::Ad2(0 | 1, ..))
    )
}

/// DBG:AD2:DCV? range,r26,n[,flags] (device test 6.5, src/ad2exp.rs
/// `dcv_test`): in plain DC V, not while logging; sets the manual DC V
/// range, runs the test, appends the AD1 means in uV (the DC V calibration
/// of the tap AD1 reads: r0 = the /100 tap of AC V 6 V, or /10 with OP2)
/// and restores the DMM row (the range stays manual: CONF:VOLT:DC = AUTO).
#[inline(never)] // .cold (link-native.x)
#[allow(clippy::too_many_arguments)]
fn dbg_dcv(app: &mut app::App, clk: &mut Clock, logger: &logger::Logger, scpi: &mut scpi::Scpi, port: &mut dyn bt::ScpiPort, r26: u8, nr: u16, flags: u8) {
    if logger.active() || app.dmm.func() != 1 {
        scpi.set_err("Settings conflict");
        return;
    }
    let r = (nr >> 8) as u8;
    app.dmm_power(clk);
    app.dmm.set_range(clk, Some(r + 1));
    let buf = port.rsp_buf();
    let (mut n, ad1) = ad2exp::dcv_test(r, r26, (nr & 0xFF) as u32, flags, buf);
    let k = if flags & ad2exp::DCV_OP2 != 0 && r == 0 { 0 } else { r.max(1) };
    for (i, c) in ad1.iter().enumerate() {
        let uv = (dmm::dcv_volts(*c as f32, k as usize) * 1e6) as i64;
        let mut t = [0u8; 24];
        let mut j = t.len();
        let mut v = uv.unsigned_abs();
        loop {
            j -= 1;
            t[j] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        if uv < 0 {
            j -= 1;
            t[j] = b'-';
        }
        j -= 1;
        t[j] = if i == 0 { b';' } else { b',' };
        let m = (t.len() - j).min(buf.len() - n);
        buf[n..n + m].copy_from_slice(&t[j..j + m]);
        n += m;
    }
    port.respond_len(n);
    app.dmm.start(clk);
    app.meter.reset();
    app.last_raw = None;
}

/// DBG:AD2:N7? item,state,n (datasheet note 7 in the stock DC rows,
/// src/ad2exp.rs `n7_test`): in DC V / DC mV, not while logging; sets the
/// MF range item (0 = 600 mV DC, 1..4 = 6 .. 1000 V; it stays manual), runs
/// the test, restores the DMM row.
#[inline(never)] // .cold (link-native.x)
#[allow(clippy::too_many_arguments)]
fn dbg_n7(app: &mut app::App, clk: &mut Clock, logger: &logger::Logger, scpi: &mut scpi::Scpi, port: &mut dyn bt::ScpiPort, item: u8, state: u8, n: u16) {
    if logger.active() || !matches!(app.dmm.func(), 1 | 3) {
        scpi.set_err("Settings conflict");
        return;
    }
    app.dmm_power(clk);
    app.dmm.set_range(clk, Some(item));
    let (m, r) = (app.dmm.mode(), app.dmm.range().max(0) as u8);
    let n = ad2exp::n7_test(item, m, r, state, n as u32, port.rsp_buf());
    port.respond_len(n);
    app.dmm.start(clk);
    app.meter.reset();
    app.last_raw = None;
}

/// A ScpiPort that notes whether a reply was queued from its buffer.
struct SentPort<'a> {
    inner: &'a mut dyn bt::ScpiPort,
    sent: bool,
}

impl bt::ScpiPort for SentPort<'_> {
    fn respond(&mut self, data: &[u8]) {
        self.sent = true;
        self.inner.respond(data)
    }
    fn rsp_buf(&mut self) -> &mut [u8] {
        self.inner.rsp_buf()
    }
    fn respond_len(&mut self, n: usize) {
        self.sent = true;
        self.inner.respond_len(n)
    }
    fn respond_with(&mut self, f: &mut dyn FnMut(&mut [u8]) -> Option<usize>) {
        let mut ok = false;
        self.inner.respond_with(&mut |b| {
            let r = f(b);
            ok = r.is_some();
            r
        });
        self.sent |= ok;
    }
    fn respond_stream(&mut self, head: &[u8], len: usize, fill: fn(usize, &mut [u8])) -> bool {
        let ok = self.inner.respond_stream(head, len, fill);
        self.sent |= ok;
        ok
    }
    fn trusted(&self) -> bool {
        self.inner.trusted()
    }
}

/// Bluetooth carries the LOG:DATA? stream up to this rate, S/s (115200
/// baud: 11.5 kB/s = 960 twelve-byte records/s, ~870 with the query
/// turnaround; docs/bt-scpi.md): every AD1 setting (<= 640/s), no AD2 rate.
const BT_MAX_HZ: u32 = 800;

/// A streamed USBTMC reply (WAV:DATA? of the scope record) that makes no
/// progress this long is given up (the host stopped reading it).
const STREAM_STALL_MS: u64 = 2000;

/// The main loop's extra USB polling per pass while a host READ of the disk
/// runs (cycles: 2 ms; 0.5 ms during a logger session).
const MSC_PUMP_CYC: u32 = clock::HCLK_HZ / 500;
const MSC_PUMP_LOG_CYC: u32 = clock::HCLK_HZ / 2000;

/// The nominal rate (S/s) of the logger's next session from `ad2` / AD1
/// (LOG:SRATe, LOG:OSR + CHOP; the stream ISR may cap AD2 lower).
fn fast_next_hz(l: &logger::Logger, ad2: bool) -> u32 {
    if ad2 {
        hy3131::ad2_rate_mhz(hy3131::ad2_r26_for(l.req_hz(), l.chop())) / 1000
    } else {
        1_000_000 / hy3131::ad1_period_us(l.osr(), l.chop()).max(1)
    }
}

/// LOG:SCALe? a,b (scpi.rs): the AD2 session's scale (value = a + b raw).
/// Region B: asked while the session streams.
#[inline(never)]
fn log_scale(scpi: &mut scpi::Scpi, l: &logger::Logger, sc: Option<dmm::Ad2Scale>) {
    scpi.len = 0;
    let sc = sc.filter(|_| l.ad2() && logger::Logger::mode_ok(true, l.mode()));
    scpi.put_f32(sc.map_or(0.0, |s| -s.offset));
    scpi.put_bytes(b",");
    scpi.put_f32(sc.map_or(0.0, |s| s.per_code));
}

/// LOG:DATA?'s compact formats: the rate and LOG:SCALe?'s a, b.
fn fmt_par(l: &logger::Logger, sc: Option<dmm::Ad2Scale>) -> logger::FmtPar {
    let sc = sc.filter(|_| l.ad2() && logger::Logger::mode_ok(true, l.mode()));
    logger::FmtPar { rate_mhz: l.rate_mhz(), a: sc.map_or(0.0, |s| -s.offset), b: sc.map_or(0.0, |s| s.per_code) }
}

/// The fast logger's rare events of this pass: Ev::Start (the logger
/// session's preparation), the session's end (toast, repaint), a Record
/// page request (Ev::Stop: the caller stopped the logger already). Returns
/// (an SCPI "Settings conflict" for both transports, start the logger
/// session now: the caller does, in region B, `fast_start`).
#[inline(never)] // .cold (link-native.x): not per pass
fn fast_events(app: &mut app::App, clk: &mut Clock, logger: &mut logger::Logger, ev: fastlog::Ev, fa: bool, was: bool, now: u64) -> (bool, bool) {
    let mut conflict = false;
    let mut go = false;
    if ev == fastlog::Ev::Start && app.inrush_saving() {
        // The inrush save: the header, then the trace drains (no logger).
        let mut txt = [0u8; 512];
        let (n, par) = app.inrush_head(&mut txt);
        fastlog::run(&fastlog::Head { par, unit: b"A", text: &txt[..n] });
    } else if matches!(ev, fastlog::Ev::Start | fastlog::Ev::Restart) {
        // Restart: the writer trial ended (fastlog `Gate`); the session
        // starts again, at 38400 when it fell back.
        go = fast_start(app, clk, logger);
        conflict = !go;
    }
    if was && !fa {
        if let Some(st) = fastlog::ended() {
            if app.inrush_saving() {
                app.inrush_saved(now, st, fastlog::status().file);
            } else {
                app.fast_ended(now, st, fastlog::status().file);
            }
        }
        dialog::log_panel_reset();
        app.invalidate();
    }
    if let Some(r) = app.take_fast_req() {
        fast_req(app, logger, r, now);
    }
    (conflict, go)
}

/// `fastlog::Ev::Start`: prepare the logger session of a disk session in
/// the current function and give fastlog its header (from `Logger::plan`);
/// true: the caller starts the logger (`Logger::start`, in region B: the
/// stream ISR may take the bus there, and no region C code may run after
/// that). False (the function no longer fits the source): the session is
/// abandoned.
#[inline(never)] // .cold (link-native.x): once per session
fn fast_start(app: &mut app::App, clk: &mut Clock, logger: &mut logger::Logger) -> bool {
    let m = app.dmm.mode();
    if logger.active() || !logger::Logger::mode_ok(logger.ad2(), m) || matches!(app.dmm.func(), 16 | 17 | dmm::FUNC_V_AUTO | dmm::FUNC_A_AUTO) || app.dmm.pw.on {
        fastlog::not_started();
        return false;
    }
    app.dmm_power(clk);
    app.dmm.start(clk);
    if logger.ad2() && fast_next_hz(logger, true) > FAST_DISK_SAFE_HZ && (!fastlog::tpp_ok() || fastlog::gate_fell()) {
        // The probe found the flash too slow for the rung (or did not run),
        // or the writer trial found the CPU short (the ring filled while
        // the stream ISR ran): the session runs at 38400, LOG:SRATe? /
        // LOG:PREamble? say so, LOG:DISK? has the measured tPP and the
        // trial's verdict.
        logger.set_req_hz(FAST_DISK_SAFE_HZ);
    }
    let _ = logger.plan(m, app.dmm.range());
    // AD2 in DC V: the self-cal now, for the header's scale (the session's
    // `start` keeps it), then the DMM row back for the disk preparation.
    logger.dc_prepare();
    let mut txt = [0u8; 512];
    let (n, par, unit) = fast_head(logger, app.dmm.ad2_scale(), &mut txt);
    fastlog::run(&fastlog::Head { par, unit, text: &txt[..n] });
    if logger.ad2() && matches!(m, 1 | 3) {
        app.dmm.start(clk);
    }
    true
}

/// The fast logger's source in a logger session (fastlog::Src): the ring.
struct LogSrc<'a>(&'a mut logger::Logger);

impl fastlog::Src for LogSrc<'_> {
    #[inline(always)]
    fn drain(&mut self, out: &mut [fastlog::Rec]) -> usize {
        self.0.drain_recs(out)
    }
    fn waiting(&self) -> u32 {
        self.0.waiting()
    }
    fn cap(&self) -> u32 {
        logger::CAP as u32
    }
    fn dropped(&self) -> u32 {
        self.0.overwritten()
    }
    fn isr_cyc(&self) -> u32 {
        self.0.isr_busy_cyc()
    }
}

/// The Record page's fast-logger request (app.rs `FastReq`).
#[inline(never)] // .cold (link-native.x): on a tap
fn fast_req(app: &mut app::App, logger: &mut logger::Logger, r: app::FastReq, now: u64) {
    match r {
        app::FastReq::Stop => fastlog::request_stop(),
        app::FastReq::Inrush => {
            let ok = !logger.active() && fastlog::begin(app::App::inrush_save_bytes(), 0).is_ok();
            app.inrush_save_begun(ok, now);
        }
        app::FastReq::Start(dur) => {
            // The source follows the function (AD2 in the AC rows, AD1 in
            // DC V / mV / A, which AD2 streams too: logger.rs `ad1_first`);
            // it stays the logger's LOG:SOURce afterwards.
            let ad2 = logger::Logger::ad1_first(app.dmm.mode()) == Some(true);
            if logger.active() {
                app.fast_ended(now, fastlog::State::Error, None);
                return;
            }
            logger.set_ad2(ad2);
            let hz = fast_next_hz(logger, ad2);
            let b = fastlog::plan_bytes(dur, hz, ad2);
            if fastlog::begin(b, dur).is_err() {
                app.fast_ended(now, fastlog::State::Error, None);
            } else {
                fast_probe_if(ad2, hz);
            }
        }
    }
}

/// LOG:STATe ON: why not (Some(error text)), Some("") = a disk session
/// began (LOG:DEST DISK), None = start the host session (the caller).
#[inline(never)] // .cold (link-native.x): once per session
fn log_on_check(app: &mut app::App, logger: &mut logger::Logger, disk: bool, dur_s: u32, trusted: bool) -> Option<&'static str> {
    let hz = fast_next_hz(logger, logger.ad2());
    if (logger.wave() && logger.active()) || fastlog::active() {
        // The DMM wave view owns the AD2 stream / a disk session runs.
        return Some("Settings conflict");
    }
    if disk && !trusted {
        // Disk writes over an open radio link: USB only (as LOG:CSV ON).
        return Some(bt::proto::REFUSED);
    }
    if !disk && !trusted && (logger.ad2() || hz > BT_MAX_HZ) {
        // 115200 baud = ~960 records/s at 12 bytes, ~870 with the query
        // turnaround (docs/bt-scpi.md).
        return Some("Rate too high for Bluetooth");
    }
    if !disk {
        return None;
    }
    if logger.active() || !logger::Logger::mode_ok(logger.ad2(), app.dmm.mode()) || matches!(app.dmm.func(), 16 | 17 | dmm::FUNC_V_AUTO | dmm::FUNC_A_AUTO) {
        return Some("Settings conflict");
    }
    Some(match fastlog::begin(fastlog::plan_bytes(dur_s, hz, logger.ad2()), dur_s) {
        Ok(()) => {
            fast_probe_if(logger.ad2(), hz);
            ""
        }
        Err(e) => e,
    })
}

/// A disk session above 38.4 kS/s (76.8: up to 225 KB/s into the flash,
/// over the W25Q128's worst-case page rate) times the chip's page program
/// at the end of Prep; `fast_start` caps the rung at 38400 when it is too
/// slow (decision 2026-10-22 #9).
fn fast_probe_if(ad2: bool, hz: u32) {
    if ad2 && hz > FAST_DISK_SAFE_HZ {
        fastlog::set_probe();
    }
}

/// The fastest AD2 rung a disk session takes without the probe: the rate
/// the flash keeps up with at the datasheet's tPP max with the measured
/// coder (docs/compact-log-format.md "Rate limits": 38400 at / over it with
/// the coder's worst case, as before).
const FAST_DISK_SAFE_HZ: u32 = 38_400;

/// LOG:DEST / LOG:DURation / LOG:DISK? (true: a reply).
/// Region B (not .cold): a host may send these while the AD2 stream runs
/// (device c92b605: LOG:DEST? / LOG:DUR? in the Wave view, late 1).
#[inline(never)]
fn fast_scpi(scpi: &mut scpi::Scpi, l: scpi::Log, disk: &mut bool, dur: &mut u32, host_session: bool) -> bool {
    scpi.len = 0;
    match l {
        scpi::Log::Dest(Some(d)) => {
            if fastlog::active() || host_session {
                scpi.set_err("Settings conflict");
            } else {
                *disk = d;
            }
            false
        }
        scpi::Log::Dest(None) => {
            scpi.put_bytes(if *disk { b"DISK" } else { b"HOST" });
            true
        }
        scpi::Log::Dur(Some(s)) => {
            *dur = s;
            false
        }
        scpi::Log::Dur(None) => {
            put_u32(scpi, *dur);
            true
        }
        _ => {
            fast_status(scpi);
            true
        }
    }
}

/// LOG:DISK? reply: state,file,alloc,bytes,rows,prep_pct,fifo_hw,programs,
/// tpp_us,tpp_max_us. Region B: a
/// host polls it while the session runs (emu/fastlogtest.py).
#[inline(never)]
fn fast_status(scpi: &mut scpi::Scpi) {
    scpi.len = 0;
    let st = fastlog::status();
    let code = match st.state {
        fastlog::State::Idle => 0,
        fastlog::State::Prep => 1,
        fastlog::State::Ready => 2,
        fastlog::State::Run => 3,
        fastlog::State::Stop => 4,
        fastlog::State::Done => 5,
        fastlog::State::Full => 6,
        fastlog::State::NoDisk => 7,
        fastlog::State::Error => 8,
    };
    put_u32(scpi, code);
    scpi.put_bytes(b",");
    match st.file {
        Some(n) => put_u32(scpi, n as u32),
        None => scpi.put_bytes(b"-1"),
    }
    // + the FIFO's high water (bytes of the 8 KiB not yet programmed: how
    // close the flash came to falling behind), the programs started (pages
    // + header pieces + partial pages; per second = the page rate) and the
    // last page-program probe (mean / max us, 0 = none; set_probe).
    let (tpp, tpp_max) = fastlog::tpp_us();
    // + the writer trial (0 none, 1 kept the rung, 2 fell back to 38400),
    // its window's stream ISR share (permille) and highest ring fill.
    // + the writer's share of the main loop's CPU (permille, the ISR out).
    for v in [st.alloc, st.bytes, st.rows, st.pct as u32, st.hw, st.programs, tpp, tpp_max, st.gate, st.gate_permille, st.gate_fill, st.writer_permille] {
        scpi.put_bytes(b",");
        put_u32(scpi, v);
    }
}

/// The fast logger's file header (fastlog.rs; kind 3, docs/compact-log-
/// format.md) of the session just started: (text length, parameters, unit).
/// AD2: value = the DMM's AD2 calibration (dmm::ad2_scale, as the wave
/// view); AD1: raw counts only.
#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
fn fast_head(l: &logger::Logger, sc: Option<dmm::Ad2Scale>, out: &mut [u8; 512]) -> (usize, dlog::FPar, &'static [u8]) {
    let ad2 = l.ad2();
    let sc = sc.filter(|_| ad2);
    let par = dlog::FPar {
        a: sc.map_or(0.0, |s| -s.offset),
        b: sc.map_or(0.0, |s| s.per_code),
        rate_mhz: l.nominal_mhz().max(1),
        flags: if sc.is_some() { dlog::FP_SCALED } else { 0 },
        src: if ad2 { 2 } else { 1 },
        mode: l.mode(),
        range: l.range(),
    };
    let unit: &'static [u8] = match sc {
        Some(s) if s.amps => b"A",
        Some(_) => b"V",
        None => b"cts",
    };
    let mut w = dlog::Txt { b: &mut out[..], n: 0 };
    w.s("# Alientek ");
    w.s(ident::model_name());
    w.s(", opendm40 ");
    w.s(ident::VERSION);
    w.s(", fast log, source ");
    w.s(if ad2 { "AD2" } else { "AD1" });
    w.s(", mode ");
    w.u(l.mode() as u64);
    w.s(", range ");
    w.u(l.range().max(0) as u64);
    w.s(", nominal ");
    w.u(l.nominal_mhz() as u64 / 1000);
    w.c(b'.');
    let f = l.nominal_mhz() % 1000;
    w.c(b'0' + (f / 100) as u8);
    w.c(b'0' + (f / 10 % 10) as u8);
    w.c(b'0' + (f % 10) as u8);
    w.s(" S/s, chop ");
    w.u(l.chop() as u64);
    if ad2 {
        w.s(", r26 ");
        w.u(l.r26() as u64);
        w.s(if l.mode() == 0 { ", AC-coupled" } else { ", DC-coupled" });
    } else {
        w.s(", osr ");
        w.u(l.osr() as u64);
    }
    w.s(if ad2 { "\r\n# raw: AD2 code, 262143 = 1.25 V\r\n" } else { "\r\n# raw: AD1 counts, uncalibrated\r\n" });
    w.s(if sc.is_some() { "t_s,raw,value_" } else { "t_s,raw,lost_before\r\n" });
    if sc.is_some() {
        w.bytes(unit);
        w.s(",lost_before\r\n");
    }
    (w.n, par, unit)
}

/// LOG:CSV:INTerval / DURation / CHANge (the Record page's settings, for
/// the next recording; a running one keeps its own: Settings conflict).
/// True: a reply in `scpi`; false: set (or refused).
#[inline(never)] // .cold (link-native.x)
fn rec_scpi(scpi: &mut scpi::Scpi, l: scpi::Log) -> bool {
    use scpi::Log::{CsvChg, CsvDur, CsvInt};
    let (ii, di) = csvlog::next();
    match l {
        CsvInt(Some(_)) | CsvDur(Some(_)) | CsvChg(Some(_)) if csvlog::active() => scpi.set_err("Settings conflict"),
        CsvInt(Some(i)) => csvlog::set_next(i, di),
        CsvDur(Some(d)) => csvlog::set_next(ii, d),
        CsvChg(Some(c)) => csvlog::set_change(c),
        _ => {
            scpi.len = 0;
            match l {
                CsvChg(_) => scpi.put_bytes(csvlog::CHANGES[csvlog::change() as usize].as_bytes()),
                CsvDur(_) => put_u32(scpi, csvlog::DURATIONS_S[di as usize]),
                _ => match csvlog::INTERVALS_S.get(ii as usize) {
                    Some(&s) => put_u32(scpi, s),
                    None => scpi.put_bytes(b"CHAN"),
                },
            }
            return true;
        }
    }
    false
}

/// Append a u32 as decimal to a SCPI reply (exact; no f32 rounding).
fn put_u32(scpi: &mut scpi::Scpi, mut v: u32) {
    let mut t = [0u8; 10];
    let mut i = t.len();
    loop {
        i -= 1;
        t[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    scpi.put_bytes(&t[i..]);
}

#[inline(never)] // .cold (link-native.x): not inlined into the hot caller
#[cfg(feature = "dbg-diag")]
fn touch_diag(
    tp: &Option<touch::Touch>,
    events: u32,
    ctl: (u8, [u8; 16]),
    loop_us: u32,
    prof: [u32; 3],
    sec: [u32; 6],
    bt_tx: (bool, u32),
) -> [u8; DIAG_LEN] {
    let mut d = [0u8; DIAG_LEN];
    // BT TX: 60 DMA path working (0 = blocking fallback), 64 bytes dropped
    // because the TX ring was full (u32).
    d[60] = bt_tx.0 as u8;
    d[64..68].copy_from_slice(&bt_tx.1.to_le_bytes());
    for (i, v) in sec.iter().enumerate() {
        d[128 + 4 * i..132 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in prof.iter().enumerate() {
        d[116 + 4 * i..120 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in scope::debug().iter().enumerate() {
        d[100 + 4 * i..104 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    d[97] = (loop_us.min(0xFFFF) & 0xFF) as u8;
    d[98] = (loop_us.min(0xFFFF) >> 8) as u8;
    d[80] = ctl.0;
    d[81..97].copy_from_slice(&ctl.1);
    if let Some(t) = tp {
        d[152] = t.ring_i;
        for (i, f) in t.ring.iter().enumerate() {
            d[153 + 7 * i..160 + 7 * i].copy_from_slice(f);
        }
        d[0..4].copy_from_slice(&t.chip);
        d[4] = 1;
        d[8..12].copy_from_slice(&events.to_le_bytes());
        d[12..19].copy_from_slice(&t.raw);
        d[20..60].copy_from_slice(&t.diag()); // touch bus / IRQ counters
    }
    d
}

/// Stop feeding the watchdog and wait for the FWDGT reset (the shim counts it
/// as a crash and boots opendm40 again).
fn wait_for_watchdog() -> ! {
    loop {
        mmio::spin(1_000_000);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Mark the panic on screen; do not feed the watchdog: the FWDGT resets and
    // the shim counts a crash and boots opendm40 again (WDT_RETRY).
    lcd::fill_rect_raw(0, lcd::H - 24, lcd::W, 24, lcd::MAGENTA);
    wait_for_watchdog()
}

// ---------------------------------------------------------------------------
// Native-format image (docs/native-boot.md "Implementation"). Watchdog policy
// (src/boot.rs): INITIAL_HOLD_MS above makes the main loop feed the FWDGT for
// as long as it runs.
// DBG:RELEASE, a panic or a hang stop the feeding -> FWDGT reset; the shim
// counts it as a crash (DBG:BOOT?; cleared after 60 s of main loop,
// boot::mark_healthy) and reboots opendm40 normally (WDT_RETRY), every time:
// no hang count leads to the loader. DBG:RESET = SYSRESETREQ ->
// normal reboot of opendm40. DBG:UPDATE zeroes the 12-byte loader marker
// (boot::enter_loader_update) and resets -> the vendor loader's update screen
// with its own USB disk until a dm40*.bin is installed. DBG:LOADER /
// DBG:RECOVER set the backup-SRAM request word LOADER_MAGIC and reset; the
// shim then drives PA2 low and jumps to the ROM (boot::loader_pa2): the same
// screen, the marker untouched.
mod board;
mod boot;
mod fatren;
mod calbackup;
mod diskbuf;
#[cfg(feature = "vdisk")]
mod vdisk;
#[cfg(feature = "vdisk")]
mod vwrite;
mod dialog;
mod screenshot;
mod scope_csv;
mod scope_wav;
mod csvlog;
mod dlog;
mod fastenc;
mod fastlog;
mod gallery;
mod fatdir;
mod setfile;
mod upload;
mod native;
mod uid;
mod recovery;
mod shim;
