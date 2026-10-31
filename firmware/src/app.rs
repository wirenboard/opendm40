//! Application: all instruments run all the time (DMM, scope, generator);
//! only the view changes. Swipe left/right cycles the views, MENU opens the
//! menu, the bottom row holds the per-view buttons.
//!
//! Side keys (touch controller key codes 0..3, stock events 0x10 << code):
//!   0 MENU: menu.  1 RUN: DMM HOLD / scope run-stop; in the DMM views held
//!   for RUN_LONG_MS = AutoHold on/off (FR-01).
//!   2 AUTO: DMM range (V / A / ohm, on release: in autorange fix the
//!   current range, then each press the next higher manual range, wrapping;
//!   held for RUN_LONG_MS: back to the default autorange, AUTO+ on V; other
//!   functions: auto range at key-down) / scope auto-set.  3 MF: DMM range
//!   page (Hold & Limits page where the function has no range page).
//! Power key PD6: double click = touch / key lock, click while locked = DMM
//! sub-mode cycle, hold 1.5 s = power off (see tick()).

use crate::clock::Clock;
use crate::dmm::{self, Button, Dmm, Reading};
use crate::gen::{self, Gen, Wave};
use crate::gesture::{self, Gesture};
use crate::meter::{Meter, Smooth};
use crate::mmio::{modify32, w32};
use crate::power::Power;
use crate::scope::{self, Scope};
use crate::ui::{self, View};

/// The stock-replica scope page (interaction, drag path, AUTO-set).
#[path = "scope_page.rs"]
mod scope_page;
/// Power meter: entry dialog, page taps, settings, SCPI (dmm::pw).
#[path = "app_power.rs"]
mod app_power;
/// The DMM wave view (scope page, source DMM: the AD2 stream).
#[path = "app_wave.rs"]
mod app_wave;
#[path = "app_press.rs"]
mod app_press;
/// The DMM as CH2 of the MCX scope page (the AD2 stream alongside).
#[path = "app_ch2.rs"]
mod app_ch2;
/// Inrush current: open / arm / close / results / save (region C) and the
/// part that runs while the AD2 stream does (region B).
#[path = "app_inrush.rs"]
mod app_inrush;
#[path = "app_inrush_live.rs"]
mod app_inrush_live;
/// The touch test page (region C).
#[path = "app_touchtest.rs"]
mod app_touchtest;
/// The protocol decoder view (region B: it runs while the scope streams).
#[path = "app_decode.rs"]
mod app_decode;
#[cfg(feature = "dec-log")]
#[path = "app_log.rs"]
mod app_log;
#[cfg(not(feature = "dec-log"))]
#[path = "app_log_off.rs"]
mod app_log;

pub use app_wave::WaveReq;

const TIM13_CCR1: u32 = 0x4000_1C34;
/// Offered stock options: dim 5/10/30 min/never, shutdown 10/30/60 min/never.
const DIM_MS: [u64; 4] = [300_000, 600_000, 1_800_000, 0];
const OFF_MS: [u64; 4] = [600_000, 1_800_000, 3_600_000, 0];
const DIM_TXT: [&str; 4] = ["5 min", "10 min", "30 min", "Never"];
const OFF_TXT: [&str; 4] = ["10 min", "30 min", "1 hour", "Never"];
/// Wake gate (stock FUN_08020248, stock/re/stock-ui-notes/interaction.md D.3):
/// after an idle dim the first touch / side key only restores the backlight,
/// and input is ignored for this long from it. opendm40 also drops the rest of
/// that waking touch or key press (its release would otherwise still tap):
/// the gate ends at the first touch-down / key-down after the window.
const WAKE_GATE_MS: u64 = 500;
const START: [View; 4] = [View::Dmm, View::Scope, View::Gen, View::Menu];
/// UF-28: the idle power-off is announced OFF_WARN_MS before (then every
/// OFF_WARN_STEP_MS): a toast and a beep at no less than ALERT_VOL %, so it
/// is heard also with the key & alert volume at 0 %.
const OFF_WARN_MS: u64 = 30_000;
const OFF_WARN_STEP_MS: u64 = 10_000;
const ALERT_VOL: u8 = 50;
/// A screenshot waits this long after a full repaint (`App::repaint_at`):
/// the DMM reading / chart / statistics come with the next reading (SLOW
/// 2.5/s; after a host LOG:STAT session the DMM restarts first), the status
/// bar and the scope trace with the next passes / frame. Device 2026-10-23:
/// a shot requested during a host session ran right after the repaint at
/// LOG:STAT OFF and held the bare page (S027: no reading, no chart).
const SHOT_SETTLE_MS: u64 = 1000;
/// A deferred toast's text, bytes (`App::toast_defer`; the toast box holds
/// ~30 characters of ASCII26).
const TOAST_LATER_MAX: usize = 48;
const TIM14: u32 = 0x4000_2000;
const BUZZER_ARR: u32 = 11200;
/// The danger alarm's low tone (danger.rs; doc 4.10): 200 MHz / 5 / 14000 =
/// 2.86 kHz, a major third under the stock 3.57 kHz.
const BUZZER_ARR_LO: u32 = 14000;
/// RUN held this long in a DMM view toggles AutoHold (the short press keeps
/// toggling HOLD at key-down, so HOLD has no added latency; the long press
/// then undoes that HOLD).
const RUN_LONG_MS: u64 = 1000;
/// The combined view's function band closes this long after the last tap.
const BAND_MS: u64 = 4000;
/// The beeper indicator stays this long after the beeper's condition last
/// held (`App::beep_ind`): its shortest on-time, and a contact that
/// chatters faster reads as one indication.
const BEEP_HOLD_MS: u64 = 150;
/// Combined view: the scope band paints at most one frame per this many ms
/// (25 fps; `App::sp_throttled`).
const COMBO_FRAME_MS: u64 = 40;
/// `App::sp_paint_pass`: a measurement cell in the same pass only after a
/// paint shorter than this (cycles, 4 ms).
const CELLS_AFTER_PAINT_CYC: u32 = crate::clock::HCLK_HZ / 250;
/// Scope drags kept for DBG:GEST?.
const GEST_N: usize = 32;

/// "Jack alerts" setting value (Settings 3/3) of an alert mask.
fn alert_mask_name(b: &mut [u8; 16], m: u8) -> &str {
    use crate::sentinel::{M_AJACK, M_JACK};
    let m = m & M_JACK;
    match m {
        M_JACK => "ON",
        M_AJACK => "A-JACK only",
        0 => "OFF",
        _ => {
            let t = b"Mask ";
            b[..5].copy_from_slice(t);
            b[5] = b'0' + (m & 7);
            core::str::from_utf8(&b[..6]).unwrap_or("")
        }
    }
}

/// The USB volume (in force, asked for) read-only (vwrite.rs, decision
/// 2026-10-22 #6); a build without `vdisk` serves the writable SPI disk.
fn usb_ro() -> (bool, bool) {
    #[cfg(feature = "vdisk")]
    return (crate::vwrite::read_only(), crate::vwrite::read_only_pending());
    #[cfg(not(feature = "vdisk"))]
    return (false, false);
}

/// Menu-like pages (not instrument views): no view swipe, MENU steps back.
/// The FR-05 rate after `r` for every rate control (the DMM page chip, a tap
/// on the value, Settings 2/3 "Rate"): resolution order FAST -> MED -> SLOW
/// -> FAST, from the stock's own behaviour (FAST: its 10/s moving reading,
/// 60000 counts, no extra digit) to the most digits. The chip and Settings
/// used to go FAST -> SLOW -> MED (the enum order; device check 540e056).
fn next_rate(r: dmm::Rate) -> dmm::Rate {
    use dmm::Rate;
    match r {
        Rate::Fast => Rate::Medium,
        Rate::Medium => Rate::Slow,
        Rate::Slow => Rate::Fast,
    }
}

fn is_page(v: View) -> bool {
    matches!(
        v,
        View::Menu
            | View::Settings
            | View::Range
            | View::Keypad
            | View::Device
            | View::Limits
            | View::Record
            | View::Gallery
            | View::Help
            | View::Inrush
            | View::TouchTest
            | View::Decode
    )
}

/// Keypad targets of the temperature page (after the generator 0..2 / 5
/// and the limits 3 / 4).
const KP_T_OFFS: u8 = 6;
const KP_NTC_R25: u8 = 7;
const KP_NTC_BETA: u8 = 8;

/// "10.000" -> "10", "4.700" -> "4.7" (keypad prefill).
// The Record page's interval list: csvlog's indices, then "Fast".
const _: () = assert!(crate::csvlog::ON_CHANGE as usize == 4 && ui::REC_FAST == 5 && ui::REC_CHANGES.len() == crate::csvlog::CHANGES.len());

/// persist W_REC bits 8..9 <-> csvlog::CHANGES index: 0 (an older record)
/// stands for the default 1 % (index 1), 1 for 0.1 %, 2 for 5 %.
fn swap01(c: u8) -> u8 {
    if c < 2 { c ^ 1 } else { c }
}

fn trim_zeros(s: &str) -> &str {
    if !s.as_bytes().contains(&b'.') {
        return s;
    }
    s.trim_end_matches('0').trim_end_matches('.')
}

/// Unit text the keypad's TEXT22 font can show (no deg / micro glyphs).
fn kp_unit_text(u: &'static str) -> &'static str {
    match u {
        "\u{b0}C" => "C",
        "\u{b0}F" => "F",
        "\u{b5}A" => "uA",
        "\u{b5}F" => "uF",
        u => u,
    }
}

pub struct App {
    pub dmm: Dmm,
    pub meter: Meter,
    pub scope: Scope,
    pub gen: Gen,
    pub view: View,
    back_view: View,
    dmm_view: ui::DmmView,
    pub bt_on: bool,
    backlight: u8,
    beep_on: bool,
    /// Stock settings (gui_v2_6.h section 2): dim / shutdown index into
    /// IDLE_MS, volume 0..100 %, start page, combined view for the menu row.
    dim_idx: usize,
    off_idx: usize,
    volume: u8,
    start_page: usize,
    /// Settings 3/3 "Mode Remember" (stock Other Settings, default OFF):
    /// a power-on continues the multimeter as it was (`restore`).
    mode_remember: bool,
    combo: View,
    /// Last touch / side key: the idle dim and the idle shutdown.
    last_input: u64,
    /// Last host activity (`note_host_activity`): the idle shutdown only.
    last_host: u64,
    /// DBG:IDLE <ms>: the shutdown time in ms instead of the Shutdown setting
    /// (0 = the setting). RAM only, for tests (emu/idletest.py).
    off_test_ms: u32,
    /// A slider (temperature offset) owns the current touch.
    /// Temperature page key drawn pressed (ui::temp_key), if any.
    tp_pressed: Option<u8>,
    /// A new NTC user R25 / Beta while that sensor is active: re-enter it
    /// (kp_apply has no clock).
    ntc_reenter: bool,
    /// Help page shown (UF-29, ui::HELP index).
    help_page: usize,
    /// Settings 3/3 "USB disk" as drawn (in force, asked for): a bus reset
    /// that applies the mode repaints the page.
    usb_ro_drawn: (bool, bool),
    /// Idle power-off warnings given since the last input (UF-28).
    off_warned: u64,
    dimmed: bool,
    /// Wake gate: start (the input that undimmed) and whether that touch /
    /// key press is still in progress, see WAKE_GATE_MS.
    wake_gate: Option<(u64, bool)>,
    pd6_since: Option<u64>,
    /// PD6 events are taken only after the key was seen released once (the
    /// stock key state starts at 4 = wait for release, FUN_0800b7e0/b87c), so
    /// the press that powered the unit on is not a click.
    pd6_armed: bool,
    /// Held >= 1.5 s: backlight off, beep, power off on release (FUN_080207e6).
    pd6_off: bool,
    /// Debounce: raw level, time it last changed, accepted level.
    pd6_raw: (bool, u64, bool),
    /// WAV:POINts (0 SCReen, u32::MAX MAXimum, else n) and WAV:FORMat WORD
    /// (scope_wav.rs; RAM only, as the other host settings).
    pub wav_points: u32,
    pub wav_word: bool,
    /// Debug: max duration (us) of scope poll / measurements / draw.
    pub prof: [u32; 3],
    persist_next: u64,
    /// Keypad target (0 frequency, 1 Vpp, 2 duty, 3 / 4 upper / lower DMM
    /// limit, 5 generator offset) and the typed text.
    kp_target: u8,
    kp_buf: [u8; 8],
    kp_n: usize,
    kp_mult: u8,
    /// Generator keypad: the text is the prefilled current value, selected
    /// (the first digit / '.' / backspace replaces it all).
    kp_sel: bool,
    /// The text was edited since the keypad opened (OK on an untouched
    /// prefill keeps the setting as it is, no re-rounding).
    kp_edit: bool,
    /// Keypad key drawn pressed since its touch-down (ui-review #11).
    kp_pressed: Option<u8>,
    /// Menu tile / list-page row or stepper key / generator box drawn
    /// pressed since its touch-down, on that view (ui-review #11).
    pressed: Option<(View, ui::RowPress)>,
    cur: ui::Cursors,
    /// Analog rail policy: false = Auto (default, stock-like: the rails the
    /// shown view needs, see update_rails), true = both rails always on.
    rails_both: bool,
    /// Host requests (SCPI / logger) that need a rail, kept until the next
    /// instrument view is shown.
    host_dmm: bool,
    /// A logger session owns the HY3131 (main.rs `busy`, set every pass):
    /// the DMM rail stays on whatever view is shown (fastlogtest --ui-nav:
    /// the Oscilloscope tile during a disk session dropped it, 0 rows).
    pub logger_busy: bool,
    /// A USB host has the meter configured (main.rs, every pass): "USB" in
    /// the status bar.
    pub usb_host: bool,
    host_scope: bool,
    /// Anti-drop: re-assert the latching V/A relay periodically (stock
    /// setting; the relay can flip on a mechanical shock).
    anti_drop: bool,
    relay_next: u64,
    meas_sel: [u8; 4],
    meas_n: usize,
    pub meas_res: crate::scope_meas::Results,
    roll: bool,
    /// Stock-replica scope page state (scope_page.rs), incl. AUTO-set.
    sp: scope_page::Sp,
    /// The wave view's next stream request for main.rs (app_wave.rs), and
    /// the DMM autorange to restore on exit (v_auto / a_auto code).
    wave_req: Option<WaveReq>,
    wave_auto: Option<u8>,
    /// A view change out of the live wave view, held until main.rs has
    /// stopped the stream (`show`, app_wave.rs `wave_exit_done`).
    wave_exit_to: Option<View>,
    /// The DMM CH2 on the MCX scope page (app_ch2.rs).
    ch2: app_ch2::Ch2,
    pub locked: bool,
    /// PD6 click timer (stock DMM key widget timer +0x48, 500 ms one-shot):
    /// a second release before it expires toggles the lock; on expiry the
    /// click cycles the DMM sub-mode while locked.
    pd6_click: Option<u64>,
    hist: [(f32, u8, &'static str); 5],
    hist_n: usize,
    hist_new: usize,
    hv_shown: bool,
    /// The beeper indicator (`beep_ind`): the last time the beeper's
    /// condition held, and the indicator as last drawn on the readout.
    beep_seen: u64,
    beep_drawn: u8,
    /// HOLD state (Meter::hold_state) | limit verdict << 8 as last painted
    /// to the DMM status label / limit badge; a mismatch means the label (and
    /// the frozen value) need a repaint. Compared in tick() so a toggle from
    /// the side key, a BT command or a limit change from SCPI repaint without
    /// a full static redraw (no framebuffer).
    hold_shown: u32,
    /// Time of the last tick (ms), for the reading path (AutoHold windows).
    now: u64,
    /// RUN key went down at this time in a DMM view, or AUTO in a function
    /// with ranges (`run_key` 1 / 2), not released yet (long press = AutoHold
    /// toggle / default autorange, see RUN_LONG_MS).
    run_down: Option<u64>,
    run_key: u8,
    /// Limits keypad: the display unit the typed value is in.
    kp_unit: &'static str,
    /// Last limit-fail beep (rate limit for a reading that hovers at a limit).
    fail_beep_at: u64,
    /// Wrong-connection alert (sentinel.rs): the assertion last toasted,
    /// the start of the power meter's current-limit beep pattern (None:
    /// silent) and whether the pattern has the buzzer on.
    alert_seq: u32,
    alert_beep_t0: Option<u64>,
    alert_buzz: bool,
    /// Danger alarm (danger.rs, doc 4.10): the event state, the setting
    /// (danger::LOUD / VOLUME / OFF, persisted), the latest reading's
    /// dangerous alert (A-JACK, A-PROT with current flowing) and since when
    /// the readings are above the 10 A rating.
    alarm: crate::danger::Alarm,
    pub alarm_mode: u8,
    danger_rd: bool,
    i10_since: Option<u64>,
    /// A fuse check was running at the last tick (`fuse_tick`).
    fuse_busy: bool,
    /// Hash of the temperature page's info line on screen (`temp_info_tick`).
    temp_info_h: u32,
    pub meas: [f32; 4],
    beep_until: u64,
    pub last_raw: Option<Reading>,
    /// Drawn instead of the meter's shown reading (dimmed, `Reading::prov`):
    /// the new function's placeholder after a switch, or a value a DMM gate
    /// still holds (`provisional`). Cleared by the next settled reading.
    prov: Option<Reading>,
    /// AUTO coupling: the mode (DC / AC part) the static view was drawn
    /// with (the bargraph's polarity; the V / A button shows the AUTO mark); 255 = not
    /// AUTO. A reading of the other part redraws it (`reading`).
    auto_drawn: u8,
    redraw: bool,
    meas_next: u64,
    pub power: Power,
    status_next: u64,
    /// Strip chart: one point per `chart_rate` x 10 ms (ui::TREND_RATES:
    /// 1/2/4/8/16 = 160/320/640 ms/div, 1.28/2.56 s/div).
    chart: [f32; ui::CHART_W as usize],
    chart_n: usize,
    chart_rate: u8,
    chart_next: u64,
    chart_unit: &'static str,
    chart_dec: u8,
    chart_dirty: bool,
    chart_cache: ui::ChartCache,
    /// Power-off confirm / toast over the view (dialog.rs); while set, the
    /// full redraw stays pending and nothing else draws.
    overlay: Option<crate::dialog::Overlay>,
    /// A toast that came while a confirm was up (`toast_defer`): its text
    /// (`toast_later_n` bytes, 0 = none) and colour, shown when the confirm
    /// has gone. The latest one wins.
    toast_later: [u8; TOAST_LATER_MAX],
    toast_later_n: u8,
    toast_later_ok: bool,
    /// A scope popup's tap is being handled: its toast waits for the popup
    /// repaint that ends the tap (`toast_later`), else the popup is drawn
    /// over it.
    toast_hold: bool,
    /// 3-finger swipe seen: take the screenshot in tick() once the view is
    /// drawn.
    shot_req: bool,
    /// Time of the last full repaint (`draw_static`): a requested shot waits
    /// SHOT_SETTLE_MS after it, so that the parts drawn by later passes (the
    /// DMM chart / statistics / status bar, the scope trace) are on the panel
    /// (device 2026-10-23: a shot deferred by a fast-log session was read
    /// right after `draw_static`, with no chart and half a status bar).
    repaint_at: u64,
    /// Combined view: the DMM function band is open until this time (0 =
    /// closed).
    band_until: u64,
    /// Meter strip: 0 / 1 (>= 30 V, HV badge) / 2 (>= 100 V) from the live
    /// value, and the REL / verdict badge alternation phase.
    hv_level: u8,
    /// Power meter: what the "I ranging" texts / the settings status line
    /// last drew (text keys, 0 = nothing; app_power.rs).
    pw_rng_key: u32,
    pw_status_key: u32,
    /// The W button's coupling as last drawn, `pw_btn_glyph`.
    pw_btn_coup: u8,
    /// Power meter coupling as last seen by `pw_tick` (255: not in the
    /// power meter) and the pending page after a change (0 none, 1 to
    /// draw, 2 drawn; app_power.rs).
    pw_coup_seen: u8,
    pw_pending: u8,
    /// Power Settings help line topic (ui::pw::HELP_*): the last tapped of
    /// the coupling row / "At load".
    pw_help: u8,
    strip_alt: bool,
    /// The last scope drags (target code, axis 0 H / 1 V, travel px, ms) for
    /// DBG:GEST?, oldest first once full (scope_page.rs).
    gest_log: [(u8, u8, i16, u16); GEST_N],
    gest_n: usize,
    /// Settings page shown (0 / 1, ui::SETTINGS / SETTINGS2).
    set_page: u8,
    /// CSV recording (csvlog.rs): the function it records (a function
    /// change ends it), and whether REC is drawn in the status block.
    rec_func: u8,
    rec_shown: bool,
    /// The REC dot's blink phase last drawn (scope_ui::rec_blink).
    rec_dim_shown: bool,
    /// Record page: free bytes on the disk as scanned on page entry
    /// (csvlog::free_bytes; Err = busy / no disk), for the estimate.
    rec_free: Result<u32, crate::csvlog::GErr>,
    /// Where MENU / Back on the Record page return to: the page it was
    /// opened from (the range page's Record button, or Hold & Limits).
    rec_back: View,
    /// Record page interval "Fast" (the fast logger, fastlog.rs; volatile:
    /// not persisted) and the session request for main.rs, which owns the
    /// logger (`take_fast_req`).
    pub rec_fast: bool,
    fast_req: Option<FastReq>,
}

/// A fast-logger request from the Record page (main.rs `take_fast_req`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FastReq {
    /// Start a session of this many seconds (0 = until stopped / full).
    Start(u32),
    Stop,
    /// Save the inrush capture (app_inrush.rs: a session fed from the trace).
    Inrush,
}

fn backlight(pct: u8) {
    unsafe { w32(TIM13_CCR1, pct.min(100) as u32) };
}

/// "V<maj>.<min>" ("V<maj>" without a minor) into `buf`, or "--" when the
/// boot-data byte is erased flash (0xFF) so the Device page never shows a bogus
/// V255.x.
fn fmt_ver(buf: &mut [u8; 16], maj: u8, min: Option<u8>) -> &str {
    if maj == 0xFF {
        return "--";
    }
    // Decimal u8, no leading zeros (at most 3 digits each, "V255.255" fits).
    fn dec(buf: &mut [u8; 16], mut n: usize, v: u8) -> usize {
        for d in [100u8, 10, 1] {
            if v >= d || d == 1 {
                buf[n] = b'0' + v / d % 10;
                n += 1;
            }
        }
        n
    }
    buf[0] = b'V';
    let mut n = dec(buf, 1, maj);
    if let Some(min) = min {
        buf[n] = b'.';
        n = dec(buf, n + 1, min);
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

fn buzzer_init() {
    unsafe {
        modify32(0x4002_3840, 0, 1 << 8); // TIM14 clock
        // PA7: AF9 (TIM14_CH1).
        modify32(0x4002_0000 + 0x20, 0xF << 28, 9 << 28);
        modify32(0x4002_0000, 3 << 14, 2 << 14);
        w32(TIM14 + 0x28, 4);
        w32(TIM14 + 0x2C, BUZZER_ARR - 1);
        w32(TIM14 + 0x18, (6 << 4) | (1 << 3));
        w32(TIM14 + 0x34, 0);
        w32(TIM14 + 0x20, 1);
        w32(TIM14 + 0x14, 1);
        w32(TIM14, 1 | (1 << 7));
    }
}

/// Buzzer duty (fraction of the period) at `vol` 0..100 % (stock: duty =
/// min(v/3, 0.99) * 0.5; 100 % = 0.495, the loudest the stock drives).
fn buzz_k(vol: u8) -> f32 {
    // `mute` (test builds): every beep and alarm at duty 0.
    if cfg!(feature = "mute") {
        return 0.0;
    }
    let v = vol as f32 / 100.0;
    (if v < 0.9 { v / 3.0 } else { v.min(0.99) }) * 0.5
}

/// Buzzer on/off at `vol` 0..100 %.
fn buzzer(on: bool, vol: u8) {
    let duty = if on { (BUZZER_ARR as f32 * buzz_k(vol)) as u32 } else { 0 };
    unsafe { w32(TIM14 + 0x34, duty) };
}

/// The danger alarm's tone (danger::tone_at: 0 off, 1 high, 2 low) at `vol`
/// (100 = Loud). Re-asserted every pass, written only when the registers
/// differ (so the emulator's write log holds the edges): a key / tap beep
/// in between cannot leave the alarm muted or at its own duty. ARR and
/// CCR1 are preloaded (CR1 ARPE, CCMR1 OC1PE: `buzzer_init`), so a tone
/// change lands whole at the next period. Tone 0 also restores the stock
/// pitch (the end of the alarm: every other beep is at BUZZER_ARR).
fn buzzer_tone(tone: u8, vol: u8) {
    let arr = if tone == 2 { BUZZER_ARR_LO } else { BUZZER_ARR };
    let ccr = if tone == 0 { 0 } else { (arr as f32 * buzz_k(vol)) as u32 };
    unsafe {
        if crate::mmio::r32(TIM14 + 0x2C) != arr - 1 {
            w32(TIM14 + 0x2C, arr - 1);
        }
        if crate::mmio::r32(TIM14 + 0x34) != ccr {
            w32(TIM14 + 0x34, ccr);
        }
    }
}

/// Finish the on-device disk writers before the power goes (power-off, the
/// reset path in main.rs): the CSV recording is stopped and committed, a
/// screenshot finished (or stopped between two slices). Never cut their FAT
/// / directory writes. A csvlog commit does not start while a screenshot
/// runs, and a screenshot waits for a commit in flight: commit what can be
/// committed, finish the screenshot, then the last commit.
pub fn finish_disk_writes(now: u64) {
    // An update-file upload holds the disk (the others wait for it): drop it
    // first, nothing of it is linked yet (upload.rs).
    crate::upload::abort();
    // A fast-logger session (fastlog.rs): its last pages, the size, the
    // trim (the logger stops with the power; the ring's rest is dropped).
    crate::fastlog::finish_blocking(5000);
    crate::csvlog::finish_blocking(now);
    crate::screenshot::finish_blocking(8000);
    crate::csvlog::finish_blocking(now);
    // An idle-time erase of a free sector (screenshot.rs `prep_finish`).
    crate::screenshot::prep_finish();
}

/// Power off as the stock: both analog rails / power hold low. The callers
/// save the settings to the backup SRAM first (App::persist_now); the copy
/// in the settings file is brought up to date here (setfile.rs), after the
/// other disk writers finished.
fn power_off() -> ! {
    finish_disk_writes(0);
    crate::setfile::flush();
    unsafe {
        w32(0x4002_1000 + 0x18, 1 << (2 + 16)); // PE2 = 0
        w32(0x4002_0800 + 0x18, 1 << (4 + 16)); // PC4 = 0
    }
    crate::wdt::powered_off()
}

/// Stored DMM function code -> the function to start in (1 = DC V for
/// anything unknown). v2 stored the HY3131 mode (0..15, no AC+DC then); v3
/// stores `Dmm::func()`: 0..15, 16 / 17 = V / A AC+DC. Both number 0..15 the
/// same way. The mV modes (2, 3) restart as AC / DC V: AUTO steps down to mV
/// again by itself, AUTO+ stays on V (UF-21). The AUTO coupling (19 / 20,
/// dmm::FUNC_*_AUTO) restarts in AUTO: the setting is the function code, in
/// the same byte, so no layout change (an older opendm40 build loads it as DC
/// V). V and A keep no coupling of their own beyond this: a tap on V / A
/// from another function starts in DC, as the stock.
fn restore_func(version: u32, code: u8) -> u8 {
    let max = if version >= 3 { 17 } else { 15 };
    match code {
        // The power meter restarts as itself (owner 2026-10-22: a full
        // function); `pw_enter` resets the energy integrator.
        dmm::pw::FUNC => dmm::pw::FUNC,
        dmm::FUNC_V_AUTO | dmm::FUNC_A_AUTO => code,
        2 | 3 => code - 2,
        c if c <= max => c,
        _ => 1,
    }
}

impl App {
    pub fn new(clk: &mut Clock) -> Self {
        scope_page::sp_init();
        app_decode::init();
        let mut dmm = Dmm::new();
        crate::ident::set_model(dmm.model(), Dmm::model_raw());
        dmm.select(clk, 1);
        buzzer_init();
        let mut a = App {
            dmm,
            meter: Meter::new(),
            scope: Scope::new(),
            gen: Gen::new(),
            view: View::Dmm,
            back_view: View::Dmm,
            dmm_view: ui::DmmView::new(),
            bt_on: true,
            backlight: 55,
            beep_on: true,
            dim_idx: 0,
            off_idx: 1,
            volume: 50,
            start_page: 0,
            mode_remember: false,
            combo: View::DmmScope,
            last_input: 0,
            last_host: 0,
            off_test_ms: 0,
            dimmed: false,
            wake_gate: None,
            pd6_since: None,
            wav_points: 0,
            wav_word: false,
            prof: [0; 3],
            persist_next: 0,
            kp_target: 0,
            kp_buf: [0; 8],
            kp_n: 0,
            kp_mult: b'H',
            kp_sel: false,
            kp_edit: false,
            kp_pressed: None,
            pressed: None,
            cur: ui::Cursors { x_on: false, y_on: false, x: [100, 300], y: [70, 210] },
            rails_both: false,
            host_dmm: false,
            usb_host: false,
            logger_busy: false,
            host_scope: false,
            anti_drop: true,
            relay_next: 0,
            meas_sel: crate::scope_meas::DEFAULT_SEL,
            meas_n: 4,
            meas_res: crate::scope_meas::Results { v: [f32::NAN; 20] },
            roll: true,
            sp: scope_page::Sp::new(),
            wave_req: None,
            wave_auto: None,
            wave_exit_to: None,
            ch2: app_ch2::Ch2::NEW,
            locked: false,
            pd6_click: None,
            hist: [(0.0, 0, ""); 5],
            hist_n: 0,
            hist_new: 0,
            hv_shown: false,
            beep_seen: 0,
            beep_drawn: 0,
            hold_shown: 0,
            now: 0,
            run_down: None,
            run_key: 1,
            kp_unit: "",
            fail_beep_at: 0,
            alert_seq: 0,
            alert_beep_t0: None,
            alert_buzz: false,
            alarm: crate::danger::Alarm::NEW,
            alarm_mode: crate::danger::LOUD,
            danger_rd: false,
            i10_since: None,
            fuse_busy: false,
            temp_info_h: 0,
            meas: [0.0; 4],
            pd6_armed: false,
            pd6_off: false,
            pd6_raw: (false, 0, false),
            beep_until: 0,
            off_warned: 0,
            help_page: 0,
            usb_ro_drawn: (true, true),
            tp_pressed: None,
            ntc_reenter: false,
            last_raw: None,
            prov: None,
            auto_drawn: 255,
            redraw: true,
            meas_next: 0,
            power: Power::new(),
            status_next: 0,
            chart: [0.0; ui::CHART_W as usize],
            chart_n: 0,
            chart_rate: ui::TREND_DEFAULT,
            chart_next: 0,
            chart_unit: "",
            chart_dec: 0,
            chart_dirty: false,
            chart_cache: ui::ChartCache::new(),
            overlay: None,
            toast_later: [0; TOAST_LATER_MAX],
            toast_later_n: 0,
            toast_later_ok: false,
            toast_hold: false,
            shot_req: false,
            repaint_at: 0,
            band_until: 0,
            hv_level: 0,
            pw_rng_key: 0,
            pw_status_key: 0,
            pw_btn_coup: 0,
            pw_coup_seen: 255,
            pw_pending: 0,
            pw_help: 0,
            strip_alt: false,
            gest_log: [(0, 0, 0, 0); GEST_N],
            gest_n: 0,
            set_page: 0,
            rec_func: 255,
            rec_shown: false,
            rec_dim_shown: false,
            rec_free: Err(crate::csvlog::GErr::NoDisk),
            rec_back: View::Limits,
            rec_fast: false,
            fast_req: None,
        };
        crate::persist::init();
        // The backup SRAM record, or the newer / only copy in the settings
        // file on the SPI disk (setfile.rs; after a power-off the backup
        // SRAM holds none). A power-off starts a new measurement (owner
        // 2026-10-25): DC V and the measurement settings at their defaults;
        // only a reset (the backup SRAM) continues them, or Mode Remember ON
        // (restore).
        if let Some(rec) = crate::setfile::pick(crate::persist::load()) {
            let from_bkp = crate::setfile::info().0 == crate::setfile::From::Bkp;
            a.restore(&rec, from_bkp && crate::persist::same_session(), clk);
        }
        backlight(a.backlight);
        let g = a.gen.s;
        a.gen.apply(gen::Settings { on: false, ..g });
        a.view = START[a.start_page];
        a.back_view = View::Dmm;
        let v = a.view;
        a.show(v, clk);
        if a.sp_on() {
            a.sp_enter(clk.now_ms());
        }
        a
    }

    fn beep(&mut self, now: u64, ms: u64) {
        if self.beep_on {
            buzzer(true, self.volume);
            self.beep_until = now + ms;
        }
    }

    #[inline(never)] // not inlined into its many callers (region B)
    fn show(&mut self, v: View, clk: &mut Clock) {
        if v != self.view && self.view == View::Scope && self.scope.wave_on() && crate::hy3131::streaming() {
            // Leaving the live wave view: the stream stops first (main.rs,
            // WaveReq::Stop, then `wave_exit_done` shows `v`). Until then
            // the view stays, so no page code (the menu's pressed feedback,
            // region C) runs while the stream ISR owns the bus (device
            // 92fb8fa: 1 late / overrun at 38400 at a MENU exit).
            self.wave_req = Some(WaveReq::Stop);
            self.wave_exit_to = Some(v);
            return;
        }
        if v != self.view {
            self.band_until = 0;
            self.sp_leave(clk.now_ms());
            if self.view == View::Scope && self.scope.wave_on() {
                // Leaving the wave view: MCX again, the DMM resumes; the
                // rails follow `v` below.
                self.wave_close(clk, false);
            }
            if self.view == View::Inrush {
                // Leaving the inrush page: the capture stops, the A
                // function's settings come back (app_inrush.rs).
                self.inrush_close(clk);
            }
            if self.view == View::Decode {
                // Leaving the decoder's frame log: it ends, the scope gets
                // its record buffer back (app_log.rs).
                self.log_close();
            }
            if matches!(v, View::Scope | View::DmmScope) {
                self.sp_enter(clk.now_ms());
            }
        }
        if is_page(v) && !is_page(self.view) {
            self.back_view = self.view;
        }
        if matches!(v, View::Dmm | View::DmmScope | View::Scope | View::Gen) {
            self.host_dmm = false;
            self.host_scope = false;
        }
        self.view = v;
        self.update_rails(clk);
        self.redraw = true;
    }

    /// Analog rails (scope.rs rails(): DMM side PE2, scope / generator side
    /// PC4). Auto policy, stock-like per view: DMM view -> DMM rail, scope /
    /// generator views -> scope rail, DMM+Scope -> both; menu pages keep what
    /// is on. Also on while needed elsewhere: DMM rail while a BT phone is
    /// linked (it polls DMM readings) or a host DMM request (SCPI MEAS / READ,
    /// LOG:STATe ON) is pending; scope rail while the generator output is on
    /// or after a host scope / generator command. "Both always" keeps both on.
    /// With its rail off the scope is paused (Scope::poll returns nothing) and
    /// the DMM is suspended (Dmm::poll returns nothing): the screen, FETCh?
    /// and BT keep the last reading taken while the rail was on. When the DMM
    /// rail comes back its row is rewritten dmm::RAIL_SETTLE_MS later (any
    /// view, any reason: a page, a BT link, a host query).
    fn update_rails(&mut self, clk: &mut Clock) {
        let (mut d, mut s) = match self.view {
            View::Dmm => (true, false),
            View::DmmScope => (true, true),
            // The limits keypad is a DMM page: keep the rails as they are.
            View::Keypad if self.kp_dmm() => (self.scope.dmm_rail(), self.scope.powered()),
            // The wave view: the DMM side only (the MCX acquisition is idle).
            View::Scope if self.scope.wave_on() => (true, false),
            // The inrush page: the DMM side (the AD2 stream, then the DMM).
            View::Inrush => (true, false),
            // The decoder's frame log: the scope input; the DMM as it was.
            View::Decode => (self.scope.dmm_rail(), true),
            View::Scope | View::Gen | View::Keypad => (false, true),
            _ => (self.scope.dmm_rail(), self.scope.powered()),
        };
        // A CSV recording (csvlog.rs) keeps the DMM measuring in every view.
        // So does the power meter (energy, the shunt state): every view.
        d |= self.host_dmm || self.logger_busy || (self.bt_on && self.power.bt_connected()) || crate::csvlog::active() || self.dmm.pw.on;
        // The DMM CH2 on the scope page streams from the DMM side.
        d |= self.ch2_want().is_some();
        s |= self.host_scope || self.gen.s.on;
        if self.rails_both {
            (d, s) = (true, true);
        }
        self.scope.rails(d, s, clk);
        let off = !self.scope.dmm_rail();
        self.dmm.set_suspended(clk, off);
    }

    /// SYST:RAIL: true = both always, false = Auto.
    pub fn set_rails_both(&mut self, both: bool, clk: &mut Clock) {
        self.rails_both = both;
        self.update_rails(clk);
    }

    /// What a page draw takes from the app state, before drawing.
    fn draw_state(&mut self) {
        self.dmm_view.label = self.t_label();
        self.auto_drawn = if self.dmm.cpl_auto() { self.dmm.shown_mode() } else { 255 };
        ui::set_cpl_auto(self.dmm.cpl_auto());
        // The power meter's AC+DC parts layout (doc 3.17): this page only.
        ui::set_pw_parts(self.view == View::Dmm && self.pw_parts_want());
        // The range pages' Record button: red while recording.
        ui::set_rec_on(crate::csvlog::active() || crate::fastlog::active());
    }

    fn draw_static(&mut self) {
        self.draw_state();
        match self.view {
            View::Dmm => {
                self.dmm_view.power = self.dmm.pw.on;
                self.pw_btn_glyph();
                self.dmm_view.draw_static(self.dmm.button(), self.dmm.shown_mode(), self.dmm.sub() == 2 || self.dmm.pw.on, self.dmm.ohm_var() == 1);
                self.pw_rng_key = 0;
                if self.dmm.pw.on {
                    ui::pw::side_clear(&mut self.dmm_view.pwc);
                    // A drawn pending page is gone: draw it again.
                    if self.pw_pending == 2 {
                        self.pw_pending = 1;
                    }
                } else if !self.dmm_view.bar_on {
                    ui::history(&self.hist, self.hist_n, self.hist_new);
                }
                self.hv_shown = false;
            }
            View::DmmScope => {
                // The stock scope page with the meter strip (scope_page.rs).
                self.dmm_view.invalidate();
                self.sp_draw_static();
                self.hv_shown = false;
            }
            View::Scope => self.sp_draw_static(),
            View::Gen => ui::gen_static(&self.gen),
            View::Keypad => {
                if matches!(self.kp_target, 3 | 4) {
                    self.limits_page();
                } else if self.kp_dmm() {
                    self.with_temp_page(None);
                } else {
                    ui::gen_static(&self.gen);
                }
                let (title, unit) = self.kp_labels();
                let n = self.kp_n;
                let t = core::str::from_utf8(&self.kp_buf[..n]).unwrap_or("");
                ui::keypad(self.kp_kind(), title, t, unit, self.kp_sel);
            }
            View::Limits => self.limits_page(),
            View::Record => self.record_page(),
            View::Inrush => self.inrush_page(),
            View::Decode => self.log_page(),
            View::Menu => ui::main_menu(ui::view_name(self.combo)),
            View::Range if self.dmm.hy_mode() == 15 => {
                self.with_temp_page(None);
                self.temp_info_tick(true);
            }
            View::Range if self.dmm.pw.on => {
                ui::pw::settings_page(&self.dmm.pw_state());
                self.pw_status_key = ui::pw::settings_status(&self.dmm.pw, 0, self.pw_help);
            }
            View::Range => {
                let b = self.dmm.button();
                let sel = self.range_sel();
                // UF-21: AUTO+ (never steps down to mV) is the recommended V
                // autorange; warn while the mV-first AUTO is in use.
                let note = (b == Button::Volt).then(|| {
                    if self.dmm.mv_auto_warn() { (dmm::MV_AUTO_WARN, true) } else { ("AUTO+ recommended: never steps to mV", false) }
                });
                let sub = if b == Button::Ohm { self.dmm.ohm_var() } else { self.dmm.sub() };
                ui::range_page(b, sel, sub, note);
                if app_wave::WAVE_BUILD && matches!(b, Button::Volt | Button::Amp) {
                    ui::wave_button();
                }
                if app_wave::WAVE_BUILD && b == Button::Amp {
                    ui::inrush_button();
                }
            }
            View::Settings if self.set_page == 1 => {
                // FR-05: rate setting and the measured readings/s.
                let mut rb = [0u8; 16];
                let rps = ui::rps_text(&mut rb, self.dmm.reading_period_ms());
                let mut vb = [0u8; 32];
                let mut n = 0;
                let applies = self.dmm.rate_applies();
                for part in [dmm::RATE_NAMES[self.dmm.rate() as usize], if applies { "  " } else { " (n/a here)" }, if applies { rps } else { "" }] {
                    let k = part.len().min(32 - n);
                    vb[n..n + k].copy_from_slice(&part.as_bytes()[..k]);
                    n += k;
                }
                let vals = [
                    core::str::from_utf8(&vb[..n]).unwrap_or(""),
                    crate::meter::SMOOTH_NAMES[self.meter.smoothing() as usize],
                    if self.dmm_view.bar_on { "Bargraph" } else { "History" },
                    if self.dmm.temp_f() { "F (Fahrenheit)" } else { "C (Celsius)" },
                    if self.sp.swipes { "Top bar" } else { "Off (stock)" },
                    if self.sp.press_fx { "ON" } else { "OFF (stock)" },
                    crate::scope_ui::TAP_NAMES[self.sp.tap_mode.min(2) as usize],
                    "", // Help row: opens the help pages (UF-29)
                    "", // More row: opens page 3
                ];
                ui::settings(1, &vals);
            }
            View::Settings if self.set_page == 2 => {
                let mut b = [0u8; 16];
                self.usb_ro_drawn = usb_ro();
                let vals = [
                    alert_mask_name(&mut b, self.dmm.alert_mask()),
                    crate::danger::NAMES[self.alarm_mode.min(2) as usize],
                    ui::usb_disk_name(self.usb_ro_drawn.0, self.usb_ro_drawn.1),
                    if self.dmm.burden_show { "ON (A, mA, uA)" } else { "OFF (stock)" },
                    "", // Device row: opens the About page, no inline value
                    "", // Touch test row: opens the page
                    if self.dmm_view.acc_on { "ON" } else { "OFF" },
                    if self.dmm.ohm_neg { "ON" } else { "OFF (stock)" },
                    if self.mode_remember { "ON" } else { "OFF" },
                ];
                ui::settings(2, &vals);
            }
            View::Settings => {
                let mut b = [[0u8; 8]; 2];
                let pct = |b: &mut [u8; 8], v: u8| -> usize {
                    let mut t = [0u8; 16];
                    let s = crate::dmm::format(&mut t, v as f32, 0);
                    b[..s.len()].copy_from_slice(s.as_bytes());
                    b[s.len()] = b'%';
                    s.len() + 1
                };
                let [b0, b1] = &mut b;
                let n0 = pct(b0, self.backlight);
                let n1 = pct(b1, self.volume);
                let grab = gesture::grab_ms();
                let mut gb = [0u8; 16];
                let grab_txt = if grab == 0 {
                    scope_page::DRAG_NAMES[self.sp.drag_mode.min(1) as usize]
                } else {
                    let mut t = [0u8; 16];
                    let s = crate::dmm::format(&mut t, grab as f32, 0);
                    gb[..5].copy_from_slice(b"Hold ");
                    gb[5..5 + s.len()].copy_from_slice(s.as_bytes());
                    gb[5 + s.len()..8 + s.len()].copy_from_slice(b" ms");
                    core::str::from_utf8(&gb[..8 + s.len()]).unwrap_or("Hold")
                };
                let vals: [&str; 9] = [
                    DIM_TXT[self.dim_idx],
                    OFF_TXT[self.off_idx],
                    core::str::from_utf8(&b0[..n0]).unwrap_or(""),
                    core::str::from_utf8(&b1[..n1]).unwrap_or(""),
                    if self.bt_on { "ON" } else { "OFF" },
                    ui::view_name(START[self.start_page]),
                    if self.rails_both { "Both always" } else { "Auto (per view)" },
                    grab_txt,
                    "", // More row: opens page 2, no inline value
                ];
                ui::settings(0, &vals);
            }
            View::Gallery => crate::gallery::get().draw(),
            View::Help => ui::help_page(self.help_page),
            View::TouchTest => self.tt_draw(),
            View::Device => {
                // Stock Device/About page (gui_v2_6.h §6): Vendor / Web / Model
                // / version lines. Only the fields opendm40 can actually source are
                // shown; stock's SN ("%s:%.8X") and "Bluetooth Name:%s" are
                // omitted (no traced derivation / no public getter).
                let model_name = match self.dmm.model() {
                    0 => "DM40A",
                    1 => "DM40B",
                    2 => "DM40C",
                    _ => "DM40?",
                };
                // Hardware / bootloader versions from the stock boot-data block
                // in internal flash (same source as the BT cmd-02 version reply,
                // bt.rs): +0x10 HW major, +0x11/+0x12 bootloader major/minor.
                // The byte->field mapping is inferred from bt.rs + README, not a
                // traced read of the stock Device page. The copy made at
                // start-up (bt::boot_ver): 0x080A4000 is in the slow flash
                // bank, and a fast-log session may stream meanwhile.
                let bd = crate::bt::boot_ver();
                let mut hwb = [0u8; 16];
                // The boot data has only a HW major: no invented ".0" minor.
                let hw = fmt_ver(&mut hwb, bd[0], None);
                let mut blb = [0u8; 16];
                let bl = fmt_ver(&mut blb, bd[1], Some(bd[2]));
                // opendm40 firmware version (NOT the stock "Software V2.6").
                let mut fwb = [0u8; 48];
                let fw = ui::device_software(&mut fwb);
                // Order and labels follow the stock §6 line order
                // (gui_v2_6.h:99: Bootloader / Software / Hardware); "Software"
                // carries the opendm40 version instead of the stock "V2.6".
                // MCU die temperature (power.rs, typical constants).
                let mut tb = [0u8; 24];
                let mut tn = 0;
                if self.power.mcu_t.is_finite() {
                    let mut t = [0u8; 16];
                    let s = dmm::format(&mut t, self.power.mcu_t, 1);
                    tb[..s.len()].copy_from_slice(s.as_bytes());
                    tn = s.len();
                    tb[tn..tn + 8].copy_from_slice(b" C (MCU)");
                    tn += 8;
                }
                let mcu = if tn > 0 { core::str::from_utf8(&tb[..tn]).unwrap_or("--") } else { "--" };
                let lines = [
                    ("Vendor", "Alientek"),
                    ("Web", "www.alientek.com"),
                    ("Model", model_name),
                    ("Bootloader", bl),
                    ("Software", fw),
                    ("Hardware", hw),
                    ("Board temp", mcu),
                ];
                ui::device_page(&lines);
            }
        }
        self.draw_reading();
    }

    /// The accuracy label's context (doc dmm-accuracy.md): None with
    /// Settings 3/3 "Accuracy" OFF or REL in force (the manual specifies a
    /// reading, not a difference of two).
    pub(crate) fn acc_ctx(&self) -> Option<ui::Acc> {
        (self.dmm_view.acc_on && !self.meter.rel_active()).then(|| ui::Acc { model: self.dmm.model(), tc: self.dmm.t_sensor() == 0 })
    }

    /// Repaint the shown DMM reading in the DMM page / the combined view's
    /// meter strip (only what changed is drawn).
    fn draw_reading(&mut self) {
        let Some(r) = self.shown_reading() else { return };
        let (hold, lim) = (self.meter.hold_state(), self.meter.verdict());
        match self.view {
            View::Dmm => {
                // DBG:PROF? "d.read" (digits + aux rows + labels).
                crate::prof::enter(crate::prof::DREAD);
                self.dmm_view.alt = self.strip_alt;
                self.dmm_view.acc = self.acc_ctx();
                self.dmm_view.reading(&r, hold, lim);
                crate::prof::exit(crate::prof::DREAD);
            }
            View::DmmScope => {
                let _ = (hold, lim);
                self.sp_strip();
            }
            _ => {}
        }
    }

    /// Status block contents (all bars).
    fn status(&self, now: u64) -> ui::Status {
        ui::Status {
            uptime_s: (now / 1000) as u32,
            bt: self.bt_on && self.power.bt_connected(),
            usb: self.usb_host,
            bars: self.power.shown_bars(self.strip_alt),
            charging: self.power.on_charger(),
            locked: self.locked,
            rec: self.meter.stats.rec,
            csv: crate::csvlog::active(),
            rec_dim: crate::scope_ui::rec_blink(now),
            page: View::CYCLE.iter().position(|&v| v == self.view).map_or(255, |i| i as u8),
        }
    }

    /// Combined view: open (or keep open) the DMM function band.
    fn band_open(&mut self, now: u64) {
        if self.band_until == 0 {
            self.pw_btn_glyph();
            ui::set_cpl_auto(self.dmm.cpl_auto());
            ui::func_band(self.dmm.button(), self.dmm.shown_mode(), self.dmm.sub() == 2, self.dmm.ohm_var() == 1);
        }
        self.band_until = now + BAND_MS;
    }

    /// Close the function band: repaint the panel and the graph under it.
    fn band_close(&mut self) {
        if self.band_until == 0 {
            return;
        }
        self.band_until = 0;
        if self.view == View::DmmScope {
            self.redraw = true;
        }
    }

    /// REL on/off (DMM page REL box, combined-view badge column), with the
    /// range hold while it is on.
    fn rel_toggle(&mut self, now: u64) {
        if self.meter.rel_active() {
            self.meter.rel_off();
            self.dmm.hold_range(false);
            self.beep(now, 300);
        } else if self.meter.rel_on(self.last_raw.as_ref()) {
            self.dmm.hold_range(self.dmm.hy_mode() <= 10 || self.dmm.hy_mode() == 14);
        }
        self.dmm_view.invalidate();
        self.chart_dirty = true;
        self.draw_reading();
    }

    /// DMM page rate chip tap: the next FR-05 conversion rate, in the same
    /// order as a tap on the value (`next_rate`: FAST -> MED -> SLOW ->
    /// FAST). Where the rate changes nothing (AC, Hz, capacitance, ...) a
    /// low beep and no change: the chip shows "n/a" there.
    fn rate_cycle(&mut self, now: u64) {
        if !self.dmm.rate_applies() {
            self.beep(now, 300);
            return;
        }
        self.dmm.set_rate(next_rate(self.dmm.rate()));
        self.chart_dirty = true;
    }

    /// Owner: a tap on the main value switches the resolution, fewer digits
    /// = faster. The steps are the FR-05 rates, in resolution order
    /// FAST -> MED -> SLOW -> FAST, with what they really give here (dmm.rs
    /// FR-05 tables, `rate_period` / `rate_window`): one reading per AD1
    /// conversion; FAST 60000 counts (DC 10/s); MED / SLOW the steady
    /// conversions averaged where the range has the +1 digit (600000
    /// counts, the last digit small). A toast says which (1.5 s;
    /// ui::rate_toast: the readings/s as the rate chip shows them and the
    /// averaging window). No rate choice (AC, Hz, cap, temperature, AC+DC):
    /// low beep + toast.
    /// The page under a toast is not drawn until it goes (`tick`), so the
    /// rate chip (the new name, "--/s" until readings at the new rate are
    /// measured: Dmm::set_rate) and the readout without the old rate's
    /// extra digit are drawn here, before the toast (device check of
    /// 0.2.1: the chip said the old rate while the toast said the new one).
    #[inline(never)] // .cold (link-native.x): a tap
    fn resolution_cycle(&mut self, now: u64) {
        if !self.dmm.rate_applies() {
            self.beep(now, 300);
            self.toast(now, "Fixed rate in this function", false);
            return;
        }
        let r = next_rate(self.dmm.rate());
        self.dmm.set_rate(r);
        self.chart_dirty = true;
        if self.view == View::Dmm {
            // The averaging window restarts (no extra digit in FAST, nor
            // before AVG_FULL conversions at MED / SLOW); a HOLD / AutoHold
            // capture keeps its value as it is.
            if let Some(mut rd) = self.shown_reading().filter(|_| matches!(self.meter.hold_state(), 0 | 2)) {
                rd.main.ext = false;
                self.dmm_view.reading(&rd, self.meter.hold_state(), self.meter.verdict());
            }
            ui::rate_chip(&mut self.chart_cache, ui::RateChip { rate: r as u8, period_ms: 0, applies: true });
        }
        let (n, ext) = self.dmm.rate_info(r);
        let mut b = [0u8; 40];
        let s = ui::rate_toast(&mut b, r as u8, self.dmm.rate_period_ms(r), n, ext);
        self.toast(now, s, true);
    }

    /// DMM function button `i` (DMM page row, combined-view band): as the
    /// stock function bar, a second tap on the current button cycles its
    /// sub-mode; then the common function-change path (`func_changed`, as
    /// `select_func`).
    fn press_func(&mut self, clk: &mut Clock, i: usize) {
        if Button::ALL[i] == Button::Power {
            // Active: Dmm::press cycles the coupling (as V / A their DC / AC /
            // AC+DC). Else the wiring dialog until it was confirmed once (owner: no
            // nagging; MF > Power Settings > Wiring shows it again).
            if !self.dmm.pw.on && !self.dmm.pw.wired {
                // From the combined view's band too: the dialog over the
                // DMM page (Cancel leaves the DMM page shown).
                if self.view != View::Dmm {
                    self.show(View::Dmm, clk);
                }
                self.pw_open_entry(clk.now_ms());
                return;
            }
        }
        self.dmm.press(clk, Button::ALL[i]);
        self.func_changed();
        self.last_raw = None;
    }

    /// The display unit for limits of the current function: the live
    /// reading's (if it is this function), else the base unit.
    fn lim_unit(&self) -> &'static str {
        let f = self.dmm.func_shown();
        match self.last_raw {
            // Power: the unit of P (the readout may show Wh / VAh).
            Some(r) if crate::meter::func_of(&r) == f => match r.power {
                Some(pv) => dmm::pw::reading_vals(&dmm::pw::PowerVals { main: dmm::pw::MAIN_P, ..pv }).0.unit,
                None => r.main.unit,
            },
            _ => match crate::meter::lim_slot(f) {
                crate::meter::LIM_POWER => "W",
                0 | 1 | 5 => "V",
                2 | 3 => "A",
                4 => "\u{3a9}",
                6 => "nF",
                7 => "Hz",
                _ => self.dmm.t_disp(0.0).1,
            },
        }
    }

    /// View::Limits: AutoHold and the limits of the current function.
    fn limits_page(&self) {
        let slot = crate::meter::lim_slot(self.dmm.func_shown());
        let l = self.meter.lim[slot];
        let unit = self.lim_unit();
        let dec = match self.last_raw {
            Some(r) if r.main.unit == unit => r.main.dec,
            _ => 3,
        };
        let fmt = |b: &mut [u8; 32], v: Option<f32>| -> usize {
            let Some(v) = v else {
                b[..2].copy_from_slice(b"--");
                return 2;
            };
            let mut t = [0u8; 16];
            let s = dmm::format(&mut t, v / crate::meter::unit_scale(unit), dec);
            let u = kp_unit_text(unit);
            let mut n = 0;
            for part in [s.as_bytes(), b" ", u.as_bytes()] {
                let k = part.len().min(32 - n);
                b[n..n + k].copy_from_slice(&part[..k]);
                n += k;
            }
            n
        };
        let (mut hb, mut lb) = ([0u8; 32], [0u8; 32]);
        let hn = fmt(&mut hb, l.hi);
        let ln = fmt(&mut lb, l.lo);
        let onoff = |b: bool| if b { "ON" } else { "OFF" };
        let vals: [&str; 9] = [
            onoff(self.meter.ahold),
            onoff(l.on),
            core::str::from_utf8(&hb[..hn]).unwrap_or(""),
            core::str::from_utf8(&lb[..ln]).unwrap_or(""),
            "",
            "",
            "",
            onoff(self.meter.lim_beep),
            ui::REC_INTERVALS[self.rec_ival()],
        ];
        let mut t = [0u8; 32];
        let pre = b"Hold & Limits: ";
        t[..pre.len()].copy_from_slice(pre);
        let name = crate::meter::LIM_NAMES[slot].as_bytes();
        t[pre.len()..pre.len() + name.len()].copy_from_slice(name);
        let title = core::str::from_utf8(&t[..pre.len() + name.len()]).unwrap_or("");
        ui::list_page(title, &ui::LIMIT_ROWS, &vals);
    }

    /// View::Record (decision 2026-10-22 #1): interval / duration of the
    /// next recording, the free-time estimate from the scan on page entry
    /// (free bytes x 2 B/row at the interval, or at the measured reading
    /// period in every-reading mode), Start / Stop.
    #[inline(never)] // .cold (link-native.x): a page draw
    fn record_page(&self) {
        let (ii, di) = crate::csvlog::next();
        if self.rec_fast {
            return self.record_page_fast(di);
        }
        // On change: at most a row per reading, as every reading.
        let row_ms = match crate::csvlog::INTERVALS_S.get(ii as usize).copied().unwrap_or(0) {
            0 => match self.dmm.reading_period_ms() {
                0 => 100,
                p => p,
            },
            s => s * 1000,
        };
        let mut b = [0u8; 32];
        let free = match self.rec_free {
            Ok(f) => {
                let h = crate::csvlog::est_hours(f, row_ms);
                let mut n = 0;
                let mut put = |p: &[u8]| {
                    let k = p.len().min(32 - n);
                    b[n..n + k].copy_from_slice(&p[..k]);
                    n += k;
                };
                if h == 0 {
                    put(b"< 1 h");
                } else {
                    let mut d = [0u8; 10];
                    let mut i = d.len();
                    let mut v = h;
                    loop {
                        i -= 1;
                        d[i] = b'0' + (v % 10) as u8;
                        v /= 10;
                        if v == 0 {
                            break;
                        }
                    }
                    put(b"about ");
                    put(&d[i..]);
                    put(b" h");
                }
                // The cap: one recording ends after 49.7 days (u32 ms).
                put(if h >= crate::csvlog::EST_CAP_H { b" (limit)" } else { b" left" });
                core::str::from_utf8(&b[..n]).unwrap_or("")
            }
            Err(crate::csvlog::GErr::Busy) => "-- (disk busy)",
            Err(_) => "-- (no disk)",
        };
        let ci = crate::csvlog::change() as usize;
        ui::record_page([ui::REC_INTERVALS[ii as usize], ui::REC_CHANGES[ci], ui::REC_DURATIONS[di as usize]], free, crate::csvlog::active());
    }

    /// The Record page's interval index (REC_FAST for the fast logger).
    fn rec_ival(&self) -> usize {
        if self.rec_fast { ui::REC_FAST } else { crate::csvlog::next().0 as usize }
    }

    /// The Record page with "Fast": the estimate at the fast logger's rate
    /// for this function (fastlog::rate_for: AD1 in DC V / mV / A, AD2 in
    /// the AC rows; none elsewhere) and its ~3 bytes per sample.
    fn record_page_fast(&self, di: u8) {
        let mut b = [0u8; 32];
        let mut n = 0;
        let mut put = |p: &[u8]| {
            let k = p.len().min(32 - n);
            b[n..n + k].copy_from_slice(&p[..k]);
            n += k;
        };
        let num = |v: u32, put: &mut dyn FnMut(&[u8])| {
            let mut d = [0u8; 10];
            let mut i = d.len();
            let mut x = v;
            loop {
                i -= 1;
                d[i] = b'0' + (x % 10) as u8;
                x /= 10;
                if x == 0 {
                    break;
                }
            }
            put(&d[i..]);
        };
        let txt = match (self.fast_rate(), self.rec_free) {
            (None, _) => "-- (DC/AC V or A only)",
            (Some(_), Err(crate::csvlog::GErr::Busy)) => "-- (disk busy)",
            (Some(_), Err(_)) => "-- (no disk)",
            (Some((hz, ad2)), Ok(f)) => {
                let s = f as u64 / (hz.max(1) as u64 * if ad2 { crate::fastlog::BPS_AD2 } else { crate::fastlog::BPS_AD1 } as u64);
                num(hz, &mut put);
                put(b"/s, ");
                if s >= 7200 {
                    num((s / 3600) as u32, &mut put);
                    put(b" h left");
                } else if s >= 120 {
                    num((s / 60) as u32, &mut put);
                    put(b" min left");
                } else {
                    // Under 2 min: seconds, not "0 min" / "1 min" (76800
                    // S/s on a 5.9 MB disk: ~25 s at 3 B/sample).
                    num(s as u32, &mut put);
                    put(b" s left");
                }
                core::str::from_utf8(&b[..n]).unwrap_or("")
            }
        };
        ui::record_page([ui::REC_INTERVALS[ui::REC_FAST], ui::REC_CHANGES[crate::csvlog::change() as usize], ui::REC_DURATIONS[di as usize]], txt, crate::fastlog::active());
    }

    /// The fast logger's (rate S/s, AD2) for the current function: AD2 in
    /// the stock AC rows, AD1 in DC V / mV / A (logger.rs `mode_ok`), at
    /// the logger's LOG:SRATe / LOG:OSR + CHOP rate (main.rs keeps
    /// fastlog::rates current); None elsewhere.
    pub fn fast_rate(&self) -> Option<(u32, bool)> {
        let m = self.dmm.mode();
        let (osr, chop, req) = crate::fastlog::cfg();
        let ad1 = 1_000_000 / crate::hy3131::ad1_period_us(osr, chop).max(1);
        let ad2 = crate::hy3131::ad2_rate_mhz(crate::hy3131::ad2_r26_for(req, chop)) / 1000;
        if matches!(self.dmm.func(), 16 | 17 | dmm::FUNC_V_AUTO | dmm::FUNC_A_AUTO) || self.dmm.pw.on {
            None
        } else {
            crate::logger::Logger::ad1_first(m).map(|a| if a { (ad2, true) } else { (ad1, false) })
        }
    }

    /// Record page Start / Stop with "Fast" (main.rs runs the fast logger:
    /// it owns the logger; its panel takes over the screen). True: the
    /// page was left / nothing more to draw.
    #[inline(never)]
    fn record_fast_tap(&mut self, now: u64, clk: &mut Clock) -> bool {
        if crate::fastlog::active() {
            self.fast_req = Some(FastReq::Stop);
            return false;
        }
        if self.fast_rate().is_none() {
            self.beep(now, 300);
            self.toast(now, "Fast: DC/AC V or A only", false);
            return true;
        }
        let di = crate::csvlog::next().1 as usize;
        self.fast_req = Some(FastReq::Start(crate::csvlog::DURATIONS_S[di]));
        self.beep(now, 60);
        let v = self.back_view;
        self.show(v, clk);
        true
    }

    /// main.rs: the Record page's fast-logger request, once.
    pub fn take_fast_req(&mut self) -> Option<FastReq> {
        self.fast_req.take()
    }

    /// A Record page request waits (`take_fast_req`).
    #[inline(always)]
    pub fn fast_req_pending(&self) -> bool {
        self.fast_req.is_some()
    }

    /// main.rs: a fast-logger session ended (`fastlog::ended`).
    pub fn fast_ended(&mut self, now: u64, st: crate::fastlog::State, file: Option<u16>) {
        use crate::fastlog::State;
        let mut t = *b"Saved FAST000.DLG";
        let (txt, ok) = match (st, file) {
            (State::Done | State::Full, Some(n)) => {
                t[10] = b'0' + (n / 100 % 10) as u8;
                t[11] = b'0' + (n / 10 % 10) as u8;
                t[12] = b'0' + (n % 10) as u8;
                (core::str::from_utf8(&t).unwrap_or("Saved"), true)
            }
            (State::Done, None) => ("Fast log cancelled", true),
            (State::Full, None) => ("Disk full", false),
            (State::NoDisk, _) => ("No disk", false),
            _ => ("Fast log failed", false),
        };
        self.toast(now, txt, ok);
        self.beep(now, if ok { 100 } else { 500 });
        self.invalidate();
    }

    /// Keypad kind for the current target (ui::KP_*).
    fn kp_kind(&self) -> u8 {
        match self.kp_target {
            0 => ui::KP_FREQ,
            1 | 2 | 5 | KP_NTC_R25 | KP_NTC_BETA => ui::KP_NUM,
            _ => ui::KP_SIGNED,
        }
    }

    /// The keypad edits a DMM setting (limits, temperature page): DMM
    /// rails, no generator status, back to that page.
    fn kp_dmm(&self) -> bool {
        matches!(self.kp_target, 3 | 4 | KP_T_OFFS | KP_NTC_R25 | KP_NTC_BETA)
    }

    /// The function has a range page (MF opens it; its Limits button
    /// opens Hold & Limits). Without one MF opens Hold & Limits directly.
    fn has_range_page(&self) -> bool {
        matches!(self.dmm.button(), Button::Volt | Button::Amp | Button::Ohm | Button::Cap | Button::Power) || self.dmm.hy_mode() == 15
    }

    /// Where MENU / Back on Hold & Limits return to: one page up, the range
    /// page it was opened from (#111), else the instrument.
    fn limits_back(&self) -> View {
        if self.has_range_page() { View::Range } else { self.back_view }
    }

    /// Open the Record page from the current page (range page, Power
    /// Settings, temperature page, Hold & Limits), which MENU / Back on it
    /// return to. The estimate's disk scan: on entry only, never while a
    /// commit holds the disk (Busy then).
    fn record_open(&mut self, clk: &mut Clock) {
        self.rec_back = self.view;
        self.rec_free = crate::csvlog::free_bytes();
        self.show(View::Record, clk);
    }

    /// Where OK / cancel / MENU on the keypad return to.
    fn kp_back(&self) -> View {
        match self.kp_target {
            3 | 4 => View::Limits,
            KP_T_OFFS | KP_NTC_R25 | KP_NTC_BETA => View::Range,
            _ => View::Gen,
        }
    }

    /// The host is talking to us (a USBTMC message, a Bluetooth SCPI line or
    /// phone-app frame, a USB disk READ / WRITE): counts as activity for the
    /// idle shutdown, so a host-driven session (SCPI polling, a firmware
    /// upload, a log or file being read out) never powers the unit off under
    /// the host. The idle dim still runs on touches and keys alone, and host
    /// activity does not undim: nobody is necessarily looking at the screen,
    /// and a dimmed backlight costs the session nothing.
    pub fn note_host_activity(&mut self, now: u64) {
        self.last_host = self.last_host.max(now);
    }

    /// The idle shutdown time in ms (0 = never): the Shutdown setting, or
    /// DBG:IDLE's test value.
    fn off_ms(&self) -> u64 {
        if self.off_test_ms != 0 {
            self.off_test_ms as u64
        } else {
            OFF_MS[self.off_idx]
        }
    }

    /// DBG:IDLE <ms> (0 = back to the Shutdown setting; `set`) and DBG:IDLE?
    /// -> <ms since the last touch / key>,<ms since the last input or host
    /// activity>,<shutdown ms, 0 = never> into `out` (emu/idletest.py).
    #[inline(never)] // its frame stays out of main.rs `run`
    pub fn dbg_idle(&mut self, set: Option<u32>, now: u64, out: &mut crate::scpi::Scpi) {
        if let Some(ms) = set {
            self.off_test_ms = ms;
            return;
        }
        let since = |t: u64| now.saturating_sub(t).min(u32::MAX as u64) as u32;
        out.len = 0;
        for (i, v) in [since(self.last_input), since(self.last_input.max(self.last_host)), self.off_ms() as u32].iter().enumerate() {
            if i > 0 {
                out.put_bytes(b",");
            }
            out.put_u32(*v);
        }
    }

    /// Repaint the whole view on the next drawing tick (main.rs: after the
    /// data-logger panel, dialog::log_panel).
    pub fn invalidate(&mut self) {
        self.redraw = true;
    }

    /// The full repaint of the view (`tick` when `redraw` is set).
    #[inline(never)] // its draw frames stay out of the callers' frames (run's)
    fn repaint(&mut self, now: u64) {
        self.redraw = false;
        // The full repaint drew any pressed item released.
        self.pressed = None;
        self.scope_invalidate();
        self.chart_cache.invalidate();
        if !self.repaint_strips() {
            // The page's background fills wait for the widgets (lcd.rs
            // "deferred fills"); main.rs flushes after the tick.
            crate::lcd::defer_start();
            self.draw_static();
        }
        // What the tick draws after the page (status, chart, reading):
        // a batch as above.
        crate::lcd::defer_start();
        self.repaint_at = now;
        self.status_next = 0;
        self.chart_dirty = true;
        // draw_static() repaints the DMM reading (shown value + HOLD/RUN
        // label) from the meter, so the label now matches.
        self.hold_shown = self.shown_state();
    }

    /// The page drawn in strips (lcd.rs "strip rendering"): each pixel
    /// reaches the panel once, top to bottom, in its final colour. Only
    /// the pages whose drawing is the same on every pass (a full-screen
    /// fill first, then widgets from the app state; their caches are reset
    /// by the draw itself); false: not such a page, or the buffer is busy
    /// (a disk job holds diskbuf::ARENA): the caller draws directly.
    #[inline(never)]
    fn repaint_strips(&mut self) -> bool {
        let ok = match self.view {
            View::Menu | View::Settings | View::Help | View::Gen | View::Scope => true,
            View::Range => self.dmm.hy_mode() != 15 && !self.dmm.pw.on,
            _ => false,
        };
        if !ok {
            return false;
        }
        let scope = self.view == View::Scope;
        if scope {
            self.draw_state();
            // The graph rows once (rendering the frame costs more than
            // drawing it), the drawing per strip.
            self.sp.uncover = 0;
            self.sp_prepare();
        }
        let Some(buf) = crate::diskbuf::strip_take() else {
            if scope {
                crate::lcd::defer_start();
                crate::scope_ui::invalidate();
                self.sp_draw_widgets();
                return true;
            }
            return false;
        };
        crate::lcd::strips(buf, || {
            if scope {
                crate::scope_ui::invalidate();
                self.sp_draw_widgets();
            } else {
                self.draw_static();
            }
        });
        if !crate::diskbuf::strip_give() {
            // A disk user took the buffer meanwhile: draw it again.
            crate::lcd::defer_start();
            self.draw_static();
        }
        true
    }

    /// Periodic work. `can_draw` is false before USB enumerated, while a
    /// screenshot is being read and for the whole of a data-logger session.
    /// Only drawing (and the 10 ms blocking relay re-assert) is skipped then:
    /// battery poll, settings save, the PD6 power key and the idle timers keep
    /// running. A no-draw period is host activity (a screenshot or a log
    /// session), so it holds off the idle shutdown: a log that runs
    /// longer than the shutdown time cannot power the unit off the moment it
    /// stops, with unread records in the ring.
    // Not inlined into main.rs `run`: its frame would add to the worst
    // stack path (it was inlined once DmmView::reading got a second caller).
    #[inline(never)]
    pub fn tick(&mut self, clk: &mut Clock, can_draw: bool) {
        let now = clk.now_ms();
        self.now = now;
        // UF-17: one bounded slice of a screenshot in flight (screenshot.rs).
        crate::screenshot::poll();
        if self.beep_until != 0 && now >= self.beep_until {
            buzzer(false, self.volume);
            self.beep_until = 0;
        }
        self.alert_beep(now);
        // The continuity / diode beeper (HY3131 CMP pin) has no loudness:
        // off at volume 0 % (Dmm::poll applies a change).
        crate::hy3131::set_beeper(self.volume > 0);
        if !can_draw {
            self.note_host_activity(now);
        }
        self.power.poll(now);
        self.persist(now);
        self.update_rails(clk);
        // Only with the DMM rail on (the relay is on the DMM side; a 10 ms
        // blocking call every 30 s on the scope page otherwise).
        if can_draw && self.anti_drop && self.scope.dmm_rail() && now >= self.relay_next {
            self.relay_next = now + 30_000;
            if self.relay_next > 30_000 {
                self.dmm.reassert_relay(clk);
            }
        }
        if self.band_until != 0 && now >= self.band_until && can_draw && self.overlay.is_none() {
            self.band_close();
        }
        // Idle dim / shutdown (stock defaults 5 min / 30 min) and the power
        // key PD6 (hold 1.5 s -> off). The dim counts touches and keys, the
        // shutdown host activity too (`note_host_activity`).
        let dim_idle = now.saturating_sub(self.last_input);
        let idle = now.saturating_sub(self.last_input.max(self.last_host));
        self.pw_tick(now);
        if DIM_MS[self.dim_idx] != 0 && dim_idle > DIM_MS[self.dim_idx] && !self.dimmed {
            self.dimmed = true;
            backlight(5);
        }
        // No idle shutdown during a MIN/MAX/AVG recording session (FR-02;
        // started explicitly, see Stats::rec). Dimming still applies.
        // Nor while the power meter integrates energy with a current flowing.
        // Nor during a CSV recording (Record page: a 24 h log on battery;
        // decisions-2026-10-22.md #1). Nor while the decoder's frame log
        // runs (app_log.rs: a session left to watch a bus).
        if self.off_ms() != 0 && idle > self.off_ms() && !self.meter.stats.rec && !self.dmm.pw.integrating() && !crate::csvlog::active() && !self.scope.log_on() {
            self.persist_now();
            power_off();
        }
        // Not while a screenshot reads the panel (its toast would land in the
        // rows not read yet; screenshot.rs "Frozen image").
        if !crate::screenshot::frozen() {
            self.off_warn(now, idle);
        }
        // RUN held in a DMM view: AutoHold on/off. The key-down already
        // toggled HOLD; set_ahold releases it again. AUTO held: the default
        // autorange.
        if self.run_down.is_some_and(|t| now.saturating_sub(t) >= RUN_LONG_MS) {
            self.run_down = None;
            if self.run_key == 2 {
                self.auto_key_end(clk, true);
            } else {
                let on = !self.meter.ahold;
                self.meter.set_ahold(on);
                self.dmm_view.invalidate();
                self.beep(now, if on { 200 } else { 60 });
                self.redraw |= self.view == View::Limits;
            }
        }
        // Power key PD6, as the stock (pressed = 1, pull-down; boot.rs
        // PD6_PRESSED_HIGH): press = activity (backlight back, FUN_08020248);
        // hold 1.5 s = backlight off + beep, power off on release
        // (FUN_080208c4 timer 0x5dc -> FUN_080207e6); release = click
        // (FUN_0803e53a): a second release within 500 ms toggles the touch /
        // key lock, a single click (500 ms timer expired, FUN_0803e000) cycles
        // the DMM sub-mode only while locked (touch is off then). The stock
        // handles the click only on the DMM screen; opendm40 toggles the lock in
        // every view and cycles the DMM sub-mode only in Dmm / DmmScope.
        // Debounce: a level counts after 20 ms stable (stock: 2 consecutive
        // 1 ms samples in the TIM7 ISR).
        let raw = unsafe { core::ptr::read_volatile(0x4002_0C10 as *const u32) } & (1 << 6) != 0;
        if raw != self.pd6_raw.0 {
            self.pd6_raw = (raw, now, self.pd6_raw.2);
        } else if now - self.pd6_raw.1 >= 20 {
            self.pd6_raw.2 = raw;
        }
        let pd6 = self.pd6_raw.2;
        if !pd6 {
            self.pd6_armed = true;
        }
        match (pd6 && self.pd6_armed, self.pd6_since) {
            (true, None) => {
                self.pd6_since = Some(now);
                self.last_input = now;
                // The power key silences the danger alarm too (doc 4.10).
                self.alarm.silence();
                if self.dimmed {
                    self.dimmed = false;
                    backlight(self.backlight);
                }
            }
            (true, Some(t)) if now - t > 1500 && !self.pd6_off => {
                self.pd6_off = true;
                backlight(0);
                self.beep(now, 100);
            }
            (false, Some(_)) => {
                self.pd6_since = None;
                if self.pd6_off {
                    self.persist_now();
                    power_off();
                }
                if self.view == View::TouchTest {
                    // The touch test page's exit (its touches and touch
                    // keys are shown, not used): no lock toggle.
                    self.pd6_click = None;
                    self.tt_close(clk);
                } else if self.pd6_click.take().is_some() {
                    self.locked = !self.locked;
                    self.status_next = 0;
                    self.beep(now, 150);
                } else {
                    self.pd6_click = Some(now + 500);
                }
            }
            _ => {}
        }
        if self.pd6_click.is_some_and(|t| now >= t) {
            self.pd6_click = None;
            if self.locked && matches!(self.view, View::Dmm | View::DmmScope) {
                let b = self.dmm.button();
                self.dmm.press(clk, b);
                self.func_changed();
                self.beep(now, 150);
            }
        }
        // A firmware file arrived on the USB disk (vwrite.rs): ask, unless
        // another confirm is up (then at its end), a screenshot is reading
        // the panel (then after it) or the data-logger panel is up (then
        // after the session: the confirm's box is taller than the panel and
        // nothing repaints the view under it during a session).
        #[cfg(feature = "vdisk")]
        if !crate::screenshot::frozen() && !crate::dialog::log_panel_shown() && !matches!(self.overlay, Some(o) if !matches!(o, crate::dialog::Overlay::Toast { .. })) {
            match crate::vwrite::take_dialog() {
                Some(true) => {
                    self.last_input = now;
                    self.sp_end_hold(now);
                    self.overlay = Some(crate::dialog::Overlay::FwInstall { until: now + crate::dialog::FW_MS });
                    self.redraw = true;
                    crate::dialog::fw_install();
                    self.beep(now, 150);
                }
                Some(false) => self.toast(now, crate::dialog::FW_BAD, false),
                None => {}
            }
        }
        if self.view == View::Settings && self.set_page == 2 && self.usb_ro_drawn != usb_ro() {
            self.redraw = true;
        }
        // A host started a logger session or the stream: the touch test
        // page (region C) closes.
        if self.view == View::TouchTest && self.tt_conflict().is_some() {
            self.set_page = 2;
            self.show(View::Settings, clk);
        }
        self.fuse_tick(clk, now);
        self.pw_entry_tick(now, can_draw);
        if self.overlay.is_some_and(|o| now >= o.until()) {
            #[cfg(feature = "vdisk")]
            if matches!(self.overlay, Some(crate::dialog::Overlay::FwInstall { .. })) {
                crate::vwrite::answer(false);
            }
            if matches!(self.overlay, Some(crate::dialog::Overlay::FuseCheck { stage: 1, .. })) {
                self.dmm.fuse_cancel(clk);
                self.func_changed();
            }
            self.overlay = None; // power-off confirm auto-cancel / toast end
            self.redraw = true;
            // A toast over the data-logger panel: the panel again.
            crate::dialog::toast_gone();
        }
        self.prov_follow();
        if !can_draw || self.overlay.is_some() {
            return;
        }
        // ---- drawing only below this point.
        if self.redraw {
            self.repaint(now);
        }
        if self.sp.uncover != 0 {
            self.sp_uncover();
        }
        // A toast that came while a confirm was up, over the page again.
        if self.toast_later_n != 0 {
            self.toast_later_show(now);
            return;
        }
        if self.shot_req && now >= self.repaint_at + SHOT_SETTLE_MS {
            self.shot_req = false;
            self.screenshot(now);
            return;
        }
        if let Some(shot) = crate::screenshot::take_result() {
            self.shot_toast(now, shot);
            return;
        }
        if let Some(st) = crate::csvlog::take_result() {
            self.rec_toast(now, st);
            return;
        }
        if crate::csvlog::active() != self.rec_shown {
            self.rec_shown = !self.rec_shown;
            self.status_next = 0;
        }
        // Gallery (FR-16): one bounded disk / drawing slice per pass.
        if self.view == View::Gallery {
            crate::gallery::get().poll();
            return;
        }
        // The inrush page: its state line / button (app_inrush_live.rs).
        if self.view == View::Inrush {
            self.inrush_tick(now);
            return;
        }
        // The decoder's frame log: changed rows and counters (app_log.rs).
        if self.view == View::Decode {
            self.log_tick(now);
            return;
        }
        // HOLD toggled (side key, or a BT HOLD command in bt::serve which we do
        // not drive here)? The reading loop skips drawing while held
        // (meter.update returns None), so repaint the frozen value and the
        // HOLD/RUN label now instead of waiting for a reading that never comes.
        if self.hold_shown != self.shown_state() {
            self.hold_shown = self.shown_state();
            self.draw_reading();
        }
        if now >= self.status_next {
            self.status_next = now + 1000;
            let st = self.status(now);
            // The 1 s alternation phase: REL / limit verdict in the strip
            // badge, A-JACK / LIVE? with HOLD in the DMM status slot and
            // the strip badge (ui::slot_alert).
            self.strip_alt = !self.strip_alt;
            match self.view {
                View::Dmm => {
                    ui::status_right(&st);
                    if ui::alert_alternates(self.meter.hold_state(), self.live_alert()) {
                        self.draw_reading();
                    }
                }
                View::Gen => ui::gen_status(&st),
                View::Keypad if !self.kp_dmm() => ui::gen_status(&st),
                View::DmmScope => self.draw_reading(),
                View::Range if self.temp_page_on() => self.temp_info_tick(false),
                _ => {}
            }
        }
        // The REC dot's blink (1 s phases): the dot's box alone.
        let dim = crate::scope_ui::rec_blink(now);
        if self.rec_shown && dim != self.rec_dim_shown {
            self.rec_dim_shown = dim;
            let st = self.status(now);
            match self.view {
                View::Dmm => ui::status_rec_blink(&st, false),
                View::Gen => ui::status_rec_blink(&st, true),
                View::Keypad if !self.kp_dmm() => ui::status_rec_blink(&st, true),
                _ => {}
            }
        }
        // Strip chart point. Gated with the drawing: while logging the DMM
        // poll is suspended, so the reading is stale and would fill the
        // chart with a flat line. The live value: the chart keeps running
        // while AutoHold freezes the readout (not in a manual HOLD).
        if now >= self.chart_next {
            self.chart_next = now + self.chart_rate as u64 * 10;
            if let Some(r) = self.meter.live().copied() {
                // An empty chart takes the unit / digits of the reading
                // even while it reads OL: the axis labels name the
                // function shown, not the previous one (device check
                // 92fb8fa: cap "0.050MOhm", diode "MOhm", temperature "Hz").
                // A range change to another unit starts the chart over in
                // the new unit, also when the new range reads OL (#93: 3.3 V
                // on 6 V -> 600 mV kept the V trace and labels). Not under
                // HOLD: the readout keeps the held unit too.
                let unit_new = r.main.unit != self.chart_unit && !self.meter.hold;
                if unit_new || (self.chart_n == 0 && (r.main.unit != self.chart_unit || r.main.dec != self.chart_dec)) {
                    self.chart_unit = r.main.unit;
                    self.chart_dec = r.main.dec;
                    self.chart_n = 0;
                    self.chart_dirty = true;
                }
                if !self.meter.hold && !r.main.ol {
                    self.chart_dec = r.main.dec;
                    let n = self.chart.len();
                    if self.chart_n == n {
                        self.chart.copy_within(1.., 0);
                        self.chart[n - 1] = r.main.v;
                    } else {
                        self.chart[self.chart_n] = r.main.v;
                        self.chart_n += 1;
                    }
                    self.chart_dirty = true;
                }
            }
        }
        // Redraw the chart for every new point (stock: 10/20/40 ms per point).
        if self.chart_dirty && self.view == View::Dmm {
            self.chart_dirty = false;
            let st = self.meter.stats_disp();
            let side = ui::Side {
                max: st.map(|s| s.1),
                avg: st.map(|s| s.2),
                min: st.map(|s| s.0),
                sd: st.map(|s| s.3),
                rel: self.meter.rel_value(),
                secs: self.meter.stats.secs(now),
                rec: self.meter.stats.rec,
            };
            let rc = ui::RateChip { rate: self.dmm.rate() as u8, period_ms: self.dmm.reading_period_ms(), applies: self.dmm.rate_applies() };
            // DBG:PROF? "d.chart" (one trend point: columns + labels + side).
            crate::prof::enter(crate::prof::DCHART);
            let pv = self.meter.live().and_then(|r| r.power).filter(|_| self.dmm.pw.on);
            // The power meter never shows the DMM column (MAX / AVG / REL /
            // MIN), also not before its first reading ("I ranging...", #96).
            ui::chart(&mut self.chart_cache, &self.chart[..self.chart_n], self.chart_dec, self.chart_rate, if self.dmm.pw.on { None } else { Some(&side) }, self.chart_unit, rc);
            if let Some(pv) = pv {
                ui::pw::side(&mut self.dmm_view.pwc, &pv);
            }
            crate::prof::exit(crate::prof::DCHART);
        }
    }

    /// What the DMM status label and limit badge show (see `hold_shown`),
    /// with the live alert (an alert that appears under HOLD shows at once).
    fn shown_state(&self) -> u32 {
        self.meter.hold_state() as u32 | (self.meter.verdict() as u32) << 8 | (self.live_alert() as u32) << 16
    }

    /// The wrong-connection alert of the live reading (sentinel.rs
    /// evaluates the hardware also while HOLD freezes the readout).
    fn live_alert(&self) -> crate::sentinel::Alert {
        self.meter.live().map_or(crate::sentinel::Alert::None, |r| r.alert)
    }

    /// The reading to draw: the shown (possibly frozen) one, with the live
    /// alert while HOLD / AutoHold hold it (decision 2026-10-22 #5: A-JACK
    /// / LIVE? alternate with HOLD in the status slot and the strip badge).
    /// A provisional reading / the placeholder (`prov`) instead, unless a
    /// HOLD / AutoHold capture freezes the readout or REL is on (the
    /// provisional value is not REL-corrected). A HOLD armed before the
    /// first settled reading (Meter::set_hold) shows `prov` with the HOLD
    /// label until that reading comes.
    pub(crate) fn shown_reading(&self) -> Option<Reading> {
        let hs = self.meter.hold_state();
        let free = matches!(hs, 0 | 2) || (hs == 1 && self.meter.shown().is_none());
        if let Some(p) = self.prov.filter(|_| free && !self.meter.rel_active()) {
            return Some(p);
        }
        let mut r = self.meter.shown().copied()?;
        if self.meter.hold_state() != 0 {
            r.alert = self.live_alert();
        }
        Some(r)
    }

    /// Make sure the DMM side is powered (SCPI / BT measurement requests).
    pub fn dmm_power(&mut self, clk: &mut Clock) {
        self.host_dmm = true;
        self.update_rails(clk);
    }

    /// X2-1 (s), Xf (Hz), Y2-1 (V) from the cursor positions (stock formulas).
    fn cursor_values(&mut self) {
        let sc = &self.scope.s;
        let t_px = scope::SDIV[sc.tb] / 40.0;
        let dx = (self.cur.x[1] as f32 - self.cur.x[0] as f32) * t_px;
        self.meas_res.v[7] = if self.cur.x_on { dx.abs() } else { f32::NAN };
        self.meas_res.v[8] = if self.cur.x_on && dx.abs() > 0.0 { 1.0 / dx.abs() } else { f32::NAN };
        let dy = -(self.cur.y[1] - self.cur.y[0]) as f32 * 4.0 * self.scope.vdiv() / 141.0;
        self.meas_res.v[17] = if self.cur.y_on { dy } else { f32::NAN };
    }

    fn cursors_from_selection(&mut self) {
        let sel = &self.meas_sel[..self.meas_n];
        self.cur.x_on = sel.contains(&7) || sel.contains(&8);
        self.cur.y_on = sel.contains(&17);
        self.scope_invalidate();
    }

    /// Open the keypad for `target`. The generator targets open with the
    /// current value (and unit) prefilled and selected (ui-review #10); the
    /// DMM limits (3 / 4) open empty.
    fn kp_open(&mut self, target: u8, clk: &mut Clock) {
        self.kp_target = target;
        self.kp_mult = b'K';
        let s = &self.gen.s;
        let mut t = [0u8; 16];
        let pre: &str = match target {
            0 => {
                // Integer in the largest unit that keeps it exact (the
                // frequency pad has no '.').
                let f = s.freq_hz;
                let (m, d) = if f >= 1_000_000 && f % 1_000_000 == 0 {
                    (b'M', 1_000_000)
                } else if f >= 1_000 && f % 1_000 == 0 {
                    (b'K', 1_000)
                } else {
                    (b'H', 1)
                };
                self.kp_mult = m;
                crate::dmm::format(&mut t, (f / d) as f32, 0)
            }
            1 => crate::dmm::format(&mut t, s.amp_v, 2),
            2 => crate::dmm::format(&mut t, s.duty as f32 / 100.0, 1),
            5 => s.offset_v.map_or("", |o| crate::dmm::format(&mut t, o, 2)),
            KP_T_OFFS => crate::dmm::format(&mut t, self.dmm.t_offs(self.dmm.t_sensor()), 1),
            KP_NTC_R25 => trim_zeros(crate::dmm::format(&mut t, self.dmm.ntc_user().0 / 1000.0, 3)),
            KP_NTC_BETA => crate::dmm::format(&mut t, self.dmm.ntc_user().1, 0),
            _ => "",
        };
        let n = pre.len().min(self.kp_buf.len() - 1);
        self.kp_buf[..n].copy_from_slice(&pre.as_bytes()[..n]);
        self.kp_n = n;
        self.kp_sel = n > 0;
        self.kp_edit = false;
        self.show(View::Keypad, clk);
    }

    fn kp_labels(&self) -> (&'static str, &'static str) {
        match self.kp_target {
            0 => ("Frequency", match self.kp_mult {
                b'M' => "MHz",
                b'K' => "KHz",
                _ => "Hz",
            }),
            1 => ("Vpp", "V"),
            2 => ("Duty", "%"),
            5 => ("Offset", "V"),
            KP_T_OFFS => ("Offset", "C"),
            KP_NTC_R25 => ("R25", "k"),
            KP_NTC_BETA => ("Beta", "K"),
            3 => ("Upper", kp_unit_text(self.kp_unit)),
            _ => ("Lower", kp_unit_text(self.kp_unit)),
        }
    }

    fn kp_apply(&mut self) {
        let n = self.kp_n;
        if self.kp_target == 5 && n == 0 {
            // Offset: OK on an empty field = auto (stock 0..Vpp).
            let st = self.gen.s.with_offset(None);
            self.gen.apply(st);
            return;
        }
        let Some(v) = crate::scpi::parse_num(&self.kp_buf[..n]) else { return };
        match self.kp_target {
            KP_T_OFFS => {
                let k = self.dmm.t_sensor();
                self.dmm.set_t_offs(k, v);
                return;
            }
            KP_NTC_R25 | KP_NTC_BETA => {
                // Limits: dmm.rs set_ntc_user (R25 100 ohm..10 Mohm, Beta
                // 1000..10000 K). Re-enter the sensor so the NTC entry range
                // (t_range0) follows a new R25.
                let (r25, b) = self.dmm.ntc_user();
                if self.kp_target == KP_NTC_R25 {
                    self.dmm.set_ntc_user(v * 1000.0, b);
                } else {
                    self.dmm.set_ntc_user(r25, v);
                }
                if self.dmm.t_sensor() == 6 {
                    self.ntc_reenter = true;
                }
                return;
            }
            _ => {}
        }
        if matches!(self.kp_target, 3 | 4) {
            // DMM limit, typed in the unit shown on the keypad; stored SI.
            // Setting a limit turns the check on.
            let si = v * crate::meter::unit_scale(self.kp_unit);
            let l = &mut self.meter.lim[crate::meter::lim_slot(self.dmm.func_shown())];
            if self.kp_target == 3 {
                l.hi = Some(si);
            } else {
                l.lo = Some(si);
            }
            l.on = true;
            self.meter.recheck();
            return;
        }
        let mut st = self.gen.s;
        match self.kp_target {
            0 => {
                let m = match self.kp_mult {
                    b'M' => 1e6,
                    b'K' => 1e3,
                    _ => 1.0,
                };
                // Keep the typed value; the output clamps it to the wave's
                // limit (gen::Gen::freq_clamped) and quantises it.
                st.freq_hz = gen::clamp_freq((v * m + 0.5).max(1.0) as u32);
            }
            1 => st = st.with_amp(v),
            5 => st = st.with_offset(Some(v)),
            _ => st.duty = (v.clamp(0.0, 100.0) * 100.0) as u16,
        }
        self.gen.apply(st);
    }

    /// Settings -> backup SRAM words.
    fn persist_words(&self) -> [u32; crate::persist::WORDS] {
        let g = &self.gen.s;
        // The MCX settings (the wave view's are not persisted).
        let sc = &self.scope.mcx_settings();
        let mut p = [0u32; crate::persist::WORDS];
        p[0] = self.dim_idx as u32
            | (self.off_idx as u32) << 4
            | (self.start_page as u32) << 8
            | (self.bt_on as u32) << 12
            | (crate::scpi::idn_emul() as u32 & 1) << 14;
        // (p[0] bit 13 was the old "both rails" flag, default 1: retired so
        // units upgraded from it start in the new Auto default.) Bit 14: SCPI
        // *IDN? emulation (SYST:IDN:EMUL 34401A; never used before, 0 = OFF).
        p[1] = self.backlight as u32 | (self.volume as u32) << 8 | (self.roll as u32) << 16;
        // The DMM function as restore_func starts it: AUTO's own steps
        // between V and mV (modes 2 / 3 <-> 0 / 1) are no settings change
        // (setfile.rs: a save per change).
        let f = match self.dmm.func() {
            f @ (2 | 3) => f - 2,
            f => f,
        };
        p[2] = f as u32 | (self.dmm.v_auto() as u32) << 8 | (self.dmm.a_auto() as u32) << 16 | (self.dmm.ohm_var() as u32) << 24;
        p[3] = Wave::ALL.iter().position(|&w| w == g.wave).unwrap_or(0) as u32 | (g.duty as u32) << 8;
        p[4] = g.freq_hz;
        p[5] = g.amp_v.to_bits();
        // FR-28 generator offset: 0 = auto (None), else |offset| with the
        // sign bit set (so 0.0 V is not 0).
        p[crate::persist::W_GEN_OFFS] = g.offset_v.map_or(0, |o| (-o.abs()).to_bits() | 0x8000_0000);
        p[6] = sc.tb as u32 | (sc.range as u32) << 8 | (sc.ac as u32) << 16 | (sc.rising as u32) << 17;
        p[7] = (sc.trig_row as u32 & 0xFFFF) | (sc.pos as u32) << 16;
        p[8] = sc.probe.to_bits();
        for i in 0..4 {
            p[9] |= (self.meas_sel[i] as u32) << (8 * i);
        }
        p[10] = self.meas_n as u32;
        // v3 words (0 = default everywhere): p[11] bit 0 rails "Both always",
        // bits 8..11 temperature sensor; p[12..15] per-sensor temperature
        // offsets (i16, 0.1 C, two per word); p[16]/p[17] user NTC R25 (ohm) /
        // Beta as f32 bits.
        p[11] = self.rails_both as u32 | (self.dmm.t_sensor() as u32) << 8 | (gesture::grab_ms() as u32) << 16;
        let o = self.dmm.t_offs_raw();
        for (i, v) in o.iter().enumerate() {
            p[12 + i / 2] |= (*v as u16 as u32) << (16 * (i % 2));
        }
        let (r25, b) = self.dmm.ntc_user();
        p[16] = r25.to_bits();
        p[17] = b.to_bits();
        // v4 words (0 = default everywhere): p[18] bit 0 AutoHold on, bit 1
        // limit fail beep OFF, bits 8..16 limits checked per slot
        // (meter::lim_slot); p[19] bits 0..8 lower limit set, bits 16..24
        // upper limit set; p[20 + 2 i] / p[21 + 2 i] lower / upper limit of
        // slot i (f32 bits, SI). p[38] persist::W_DMM_EXT, p[39]
        // persist::W_GEN_OFFS.
        // p[6] bits 18..27: scope trigger position + 1 (0 = centre, older
        // records); p[11] bits 16..31: scope drag hold-to-grab ms (0 = direct).
        let m = &self.meter;
        p[18] = m.ahold as u32 | (!m.lim_beep as u32) << 1;
        // v10: the power slot's values at persist::W_LIM_W (its bits as the
        // other slots': p[18] bit 17, p[19] bits 9 / 25).
        for (i, l) in m.lim.iter().enumerate() {
            let w = if i == crate::meter::LIM_POWER { crate::persist::W_LIM_W } else { 20 + 2 * i };
            p[18] |= (l.on as u32) << (8 + i);
            p[19] |= (l.lo.is_some() as u32) << i | (l.hi.is_some() as u32) << (16 + i);
            p[w] = l.lo.unwrap_or(0.0).to_bits();
            p[w + 1] = l.hi.unwrap_or(0.0).to_bits();
        }
        p[6] |= (sc.hpos as u32 + 1) << 18;
        // Scope page: colour, brightness, tap-step mode (p[1] bits 17..29),
        // drag mode, swipes, persistence, pressed feedback, grid intensity
        // (p[11] bits 1..6, 12..15); 0 = default everywhere.
        let (mut p1, mut p11) = (p[1], p[11]);
        self.sp_persist(&mut p1, &mut p11);
        p[1] = p1;
        p[11] = p11;
        self.sp_persist_ext(&mut p[crate::persist::W_SCOPE_EXT..]);
        // FR-05 rate + smoothing, UF-21 "V autorange flag is trusted" bit.
        let w = self.pw_words();
        p[crate::persist::W_POWER..crate::persist::W_POWER + app_power::PW_WORDS].copy_from_slice(&w);
        let z = self.pw_zd_words();
        p[crate::persist::W_POWER_ZD..crate::persist::W_POWER_ZD + app_power::PW_ZD_WORDS].copy_from_slice(&z);
        p[crate::persist::W_DMM_EXT] = crate::meter::ext_word(self.dmm.rate(), self.meter.smoothing())
            | if self.dmm_view.bar_on { 0 } else { crate::meter::EXT_HISTORY_ROW }
            | if self.dmm.temp_f() { crate::meter::EXT_TEMP_F } else { 0 }
            | ((!self.dmm.alert_mask() & crate::sentinel::M_JACK) as u32) << crate::meter::EXT_ALERT_OFF_SHIFT
            | if self.dmm.burden_show { crate::meter::EXT_BURDEN } else { 0 }
            | if self.dmm_view.acc_on { 0 } else { crate::meter::EXT_ACC_OFF }
            | if self.dmm.ohm_neg { crate::meter::EXT_OHM_NEG } else { 0 }
            | (self.alarm_mode.min(2) as u32) << crate::meter::EXT_DANGER_SHIFT
            | if self.mode_remember { crate::meter::EXT_MODE_REMEMBER } else { 0 }
            | ui::trend_code(self.chart_rate) << crate::meter::EXT_TREND_SHIFT
            | self.fixed_range_item().map_or(0, |i| i as u32 + 1) << crate::meter::EXT_RANGE_SHIFT;
        let (ri, rd) = crate::csvlog::next();
        p[crate::persist::W_REC] = ri as u32 | (rd as u32) << 4 | (swap01(crate::csvlog::change()) as u32) << 8;
        // v11: the touch calibration (touchcal.rs; 0 = none).
        for (w, c) in p[crate::persist::W_TOUCH_CAL..crate::persist::W_TOUCH_CAL + 6].iter_mut().zip(crate::touch::cal()) {
            *w = c as u32;
        }
        p
    }

    /// The range page item of a range the user fixed in V, A or ohm (the
    /// settings: Mode Remember restores it). None in autorange, under REL
    /// (its range hold ends with REL), in the AUTO coupling and the power
    /// meter.
    fn fixed_range_item(&self) -> Option<u8> {
        if self.dmm.is_auto() || self.meter.rel_active() || self.dmm.cpl_auto() || self.dmm.pw.on {
            return None;
        }
        let b = self.dmm.button();
        if !matches!(b, Button::Volt | Button::Amp | Button::Ohm) {
            return None;
        }
        let code = ui::range_code(self.dmm.hy_mode(), self.dmm.range().max(0) as u8);
        ui::range_items(b).iter().any(|it| it.1 == Some(code)).then_some(code)
    }

    /// Backup SRAM words -> settings. `same_session`: after a normal
    /// opendm40 boot (persist::same_session) that kept the backup SRAM (a
    /// reset, not a power-off) the measurement continues: the DMM function
    /// (an unknown code reads as DC V), its ranges / AUTO flags, AutoHold,
    /// the limits, rate and smoothing, the scope's acquisition, trigger,
    /// math and measurements, the power meter and the Record page. Otherwise
    /// only the preferences are restored (display, sound, power, BT, alerts,
    /// units, sensor, scope look and touch, the touch calibration, the
    /// generator's setup) and the rest keeps its defaults, as the stock
    /// after a power-on. Mode Remember ON (the stock's Other Settings
    /// switch; its record: function, coupling, autorange flags, ohm variant,
    /// chart rate) continues the multimeter after a power-off too: the
    /// function and coupling, the AUTO / AUTO+ choices and the ohm test
    /// current, a fixed V / A / ohm range, rate and smoothing, the trend
    /// timebase. Not AutoHold, the limits, REL or HOLD (as the stock: they
    /// change what the readout shows, unnoticed after a power-on), nor the
    /// scope, the power meter's settings or the Record page.
    fn restore(&mut self, rec: &crate::persist::Record, same_session: bool, clk: &mut Clock) {
        let p = &rec.words;
        self.mode_remember = p[crate::persist::W_DMM_EXT] & crate::meter::EXT_MODE_REMEMBER != 0;
        let cont = same_session || self.mode_remember;
        self.dim_idx = (p[0] & 3) as usize;
        self.off_idx = (p[0] >> 4 & 3) as usize;
        self.start_page = (p[0] >> 8 & 3) as usize;
        self.bt_on = p[0] >> 12 & 1 != 0;
        crate::scpi::set_idn_emul((p[0] >> 14 & 1) as u8);
        self.backlight = (p[1] & 0xFF).clamp(10, 100) as u8;
        self.volume = (p[1] >> 8 & 0xFF).min(100) as u8;
        crate::hy3131::set_beeper(self.volume > 0); // before the first DMM row
        self.roll = p[1] >> 16 & 1 != 0;
        self.scope.roll = self.roll;
        self.rails_both = p[11] & 1 != 0;
        self.sp_restore(p[1], p[11]);
        if same_session {
            self.sp_restore_ext(&p[crate::persist::W_SCOPE_EXT..]);
        }
        gesture::set_grab_ms((p[11] >> 16) as u16);
        let mut o = [0i16; 7];
        for (i, v) in o.iter_mut().enumerate() {
            *v = (p[12 + i / 2] >> (16 * (i % 2))) as u16 as i16;
        }
        self.dmm.set_t_offs_raw(o);
        if p[16] != 0 {
            self.dmm.set_ntc_user(f32::from_bits(p[16]), f32::from_bits(p[17]));
        }
        // v4: AutoHold, limits (all zero in older records = off / unset).
        let m = &mut self.meter;
        m.lim_beep = p[18] & 2 == 0;
        if same_session {
            m.ahold = p[18] & 1 != 0;
            for (i, l) in m.lim.iter_mut().enumerate() {
                let f = |w: u32, set: bool| Some(f32::from_bits(w)).filter(|v| set && v.is_finite());
                let w = if i == crate::meter::LIM_POWER { crate::persist::W_LIM_W } else { 20 + 2 * i };
                l.on = p[18] >> (8 + i) & 1 != 0;
                l.lo = f(p[w], p[19] >> i & 1 != 0);
                l.hi = f(p[w + 1], p[19] >> (16 + i) & 1 != 0);
            }
        }
        // Sensor and autorange flags before the mode, so a restored mode
        // enters with them.
        self.dmm.set_t_sensor(clk, (p[11] >> 8 & 0xF) as u8);
        // FR-05 rate / smoothing; UF-21: a V autorange stored before the
        // AUTO+ default (no "trusted" bit in W_DMM_EXT) restores as AUTO+.
        let ext = crate::meter::ext_settings(p[crate::persist::W_DMM_EXT]);
        if cont {
            self.dmm.set_rate(ext.rate);
            self.meter.set_smoothing(ext.smooth);
            self.chart_rate = ui::trend_of_code(p[crate::persist::W_DMM_EXT] >> crate::meter::EXT_TREND_SHIFT & 7);
            let v_auto = if ext.v_auto_valid { (p[2] >> 8) as u8 } else { 1 };
            self.dmm.set_auto_flags(v_auto, (p[2] >> 16) as u8, (p[2] >> 24) as u8);
        }
        self.dmm_view.bar_on = p[crate::persist::W_DMM_EXT] & crate::meter::EXT_HISTORY_ROW == 0;
        self.dmm.set_temp_f(p[crate::persist::W_DMM_EXT] & crate::meter::EXT_TEMP_F != 0);
        // Before the first row is written (select below).
        // (Bit 15, the removed "Leads in A?" reminder's OFF, is ignored.)
        let off = (p[crate::persist::W_DMM_EXT] >> crate::meter::EXT_ALERT_OFF_SHIFT) as u8 & crate::sentinel::M_JACK;
        self.dmm.sent.set_mask(!off & crate::sentinel::M_ALL);
        self.dmm.burden_show = p[crate::persist::W_DMM_EXT] & crate::meter::EXT_BURDEN != 0;
        self.dmm_view.acc_on = p[crate::persist::W_DMM_EXT] & crate::meter::EXT_ACC_OFF == 0;
        self.dmm.ohm_neg = p[crate::persist::W_DMM_EXT] & crate::meter::EXT_OHM_NEG != 0;
        // An unknown value (3) reads as Loud.
        self.alarm_mode = (p[crate::persist::W_DMM_EXT] >> crate::meter::EXT_DANGER_SHIFT & 3) as u8 % 3;
        if same_session {
            self.pw_restore(&p[crate::persist::W_POWER..crate::persist::W_POWER + app_power::PW_WORDS]);
            self.pw_zd_restore(&p[crate::persist::W_POWER_ZD..crate::persist::W_POWER_ZD + app_power::PW_ZD_WORDS]);
            // v8: Record page interval / duration (0 = the defaults; csvlog
            // maps an unknown index to the default).
            crate::csvlog::set_next((p[crate::persist::W_REC] & 0xF) as u8, (p[crate::persist::W_REC] >> 4 & 0xF) as u8);
            crate::csvlog::set_change(swap01((p[crate::persist::W_REC] >> 8 & 3) as u8));
        }
        let mode = if cont { restore_func(rec.version, p[2] as u8) } else { 1 };
        // Entering the power meter starts in DC (as V / A); a restart in it
        // keeps its coupling, as V / A keep theirs in the function code.
        let coup = self.dmm.pw.coup;
        self.dmm.select(clk, mode);
        if self.dmm.pw.on {
            self.dmm.pw_set_coupling(clk, coup);
        }
        // Mode Remember: the fixed range of that function (a reset without
        // it starts in autorange, as before).
        let item = (p[crate::persist::W_DMM_EXT] >> crate::meter::EXT_RANGE_SHIFT & 0xF) as u8;
        if self.mode_remember && item != 0 && !self.dmm.cpl_auto() && !self.dmm.pw.on {
            let b = self.dmm.button();
            if matches!(b, Button::Volt | Button::Amp | Button::Ohm) && ui::range_items(b).iter().any(|it| it.1 == Some(item - 1)) {
                self.dmm.set_range(clk, Some(item - 1));
            }
        }
        let mut g = self.gen.s;
        g.wave = Wave::ALL[(p[3] & 3) as usize];
        g.duty = ((p[3] >> 8) & 0xFFFF).min(10000) as u16;
        g.freq_hz = gen::clamp_freq(p[4]);
        let a = f32::from_bits(p[5]);
        g.amp_v = if a.is_finite() { a.clamp(gen::AMP_MIN, gen::AMP_MAX) } else { 3.0 };
        let o = p[crate::persist::W_GEN_OFFS];
        g.offset_v = if o == 0 { None } else { Some(f32::from_bits(o & 0x7FFF_FFFF)) };
        self.gen.s = g.normalised();
        // v11: the touch calibration, a preference (0 = none; one that
        // fails the checks reads as none).
        let mut tc = [0i32; 6];
        for (c, w) in tc.iter_mut().zip(&p[crate::persist::W_TOUCH_CAL..]) {
            *c = *w as i32;
        }
        app_touchtest::tc_restore(tc);
        if !same_session {
            return;
        }
        let mut sc = self.scope.s;
        sc.tb = (p[6] & 0xFF) as usize;
        sc.range = (p[6] >> 8 & 0xFF) as usize;
        sc.ac = p[6] >> 16 & 1 != 0;
        sc.rising = p[6] >> 17 & 1 != 0;
        sc.trig_row = (p[7] & 0xFFFF) as u16 as i16 as i32;
        sc.pos = (p[7] >> 16) as u16 as i16 as i32;
        let pr = f32::from_bits(p[8]);
        sc.probe = if scope::PROBES.contains(&pr) { pr } else { 1.0 };
        let h = (p[6] >> 18 & 0x3FF) as i32;
        sc.hpos = if h == 0 { 200 } else { h - 1 };
        // The saved level row is at the saved scale: its volts from there.
        self.scope.levels_reset();
        self.scope.apply(sc, 0);
        let n = (p[10] as usize).min(4);
        // Only valid measurement ids (0..19, index into meas_res.v), no
        // duplicates (review 7389e8a L1).
        let mut k = 0;
        for i in 0..n {
            let id = (p[9] >> (8 * i)) as u8;
            if id < 20 && !self.meas_sel[..k].contains(&id) {
                self.meas_sel[k] = id;
                k += 1;
            }
        }
        self.meas_n = k;
        self.cursors_from_selection();
    }

    /// Save the settings now and then (cheap: 64 words of backup SRAM; the
    /// SPI-disk copy follows from main.rs, setfile::poll).
    pub fn persist(&mut self, now: u64) {
        if now >= self.persist_next {
            self.persist_next = now + 2000;
            self.persist_now();
        }
    }

    /// The settings into the backup SRAM at once (before a power-off). Not
    /// inlined: the 256-byte word array is gone before power_off's disk
    /// writes run.
    #[inline(never)]
    fn persist_now(&self) {
        let w = self.persist_words();
        crate::persist::save(&w);
    }

    /// Index of the current range in the MF page list.
    fn range_sel(&self) -> Option<usize> {
        let b = self.dmm.button();
        if !self.dmm.is_auto() {
            return self.range_sel_manual();
        }
        let want: Option<u8> = match b {
            Button::Volt if self.dmm.v_auto() == 1 => Some(255),
            Button::Amp if self.dmm.a_auto() == 1 => Some(255),
            _ => None,
        };
        ui::range_items(b).iter().position(|it| it.1 == want)
    }

    /// Index in the MF page list of the manual item for the range the DMM
    /// is in now (also while it autoranges).
    fn range_sel_manual(&self) -> Option<usize> {
        let want = Some(ui::range_code(self.dmm.hy_mode(), self.dmm.range() as u8));
        ui::range_items(self.dmm.button()).iter().position(|it| it.1 == want)
    }

    /// Temperature sensor name for the DMM status bar (temperature only).
    fn t_label(&self) -> Option<&'static str> {
        if self.dmm.hy_mode() == 15 { Some(ui::T_LABELS[self.dmm.t_sensor() as usize]) } else { None }
    }

    /// The AUTO coupling's provisional reading (a measured part, before its
    /// first decision): what READ? / MEAS? answer when AUTO_WAIT_MS runs out
    /// (main.rs). None for the placeholder and outside AUTO.
    pub fn auto_prov(&self) -> Option<Reading> {
        self.prov.filter(|p| p.cpl_auto && self.dmm.cpl_auto() && !p.main.v.is_nan())
    }

    /// The continuity / diode beeper indicator (ui::SHORT_STYLE: a green
    /// SHORT / BEEP band over the aux row, the digits green), after every
    /// DMM poll (5 ms; `polled` false while the logger owns the HY3131: no
    /// condition, off). On at once with the condition (`Dmm::beeping`:
    /// continuity's comparator, the diode pattern), off BEEP_HOLD_MS after
    /// it last held: a chattering contact reads as one indication.
    pub fn beep_ind(&mut self, now: u64, can_draw: bool, polled: bool) {
        let k = if polled { self.dmm.beeping() } else { 0 };
        let want = if k != 0 {
            self.beep_seen = now;
            k
        } else if self.dmm_view.short != 0 && polled && now < self.beep_seen + BEEP_HOLD_MS {
            self.dmm_view.short
        } else {
            0
        };
        self.dmm_view.short = want;
        if !can_draw || self.redraw {
            return;
        }
        match self.view {
            View::Dmm => {
                self.dmm_view.short_sync();
                if self.beep_drawn != want {
                    self.beep_drawn = want;
                    self.draw_reading();
                }
            }
            View::DmmScope if self.beep_drawn != want => {
                self.beep_drawn = want;
                self.sp_strip();
            }
            _ => self.beep_drawn = want,
        }
    }

    /// A provisional DMM reading (`Reading::prov`, switch-latency.md
    /// proposal 6): shown dimmed in place of the readout until the next
    /// settled reading; never reaches the meter (statistics, REL, limits,
    /// trend, CSV, beeps), BT (`last_raw`) or SCPI.
    pub fn provisional(&mut self, r: Reading, can_draw: bool) {
        self.prov = Some(r);
        if can_draw && !self.redraw && matches!(self.view, View::Dmm | View::DmmScope) {
            self.draw_reading();
        }
    }

    /// New DMM reading (always measured, whatever the view). Returns the
    /// live reading (REL applied) for SCPI READ? / MEAS?, also while HOLD or
    /// AutoHold freeze the readout (before, a HOLD made READ? wait forever).
    pub fn reading(&mut self, raw: Reading, can_draw: bool) -> Option<Reading> {
        self.last_raw = Some(raw);
        // A placeholder / provisional value on screen: this settled reading
        // replaces it now (not after the draw-rate limit).
        let had_prov = self.prov.take().is_some();
        if had_prov {
            self.meter.show_now();
        }
        if raw.cpl_auto && self.auto_drawn != 255 && matches!(self.auto_drawn, 0 | 2 | 4..=6) != raw.is_ac() && self.view == View::Dmm {
            // AUTO coupling switched the part shown: the bargraph's
            // polarity follows (static redraw of the DMM page; the V / A
            // button keeps the AUTO glyph, the glyph above the unit is the
            // reading's); REL and the statistics restart in Meter::update.
            self.redraw = true;
        }
        self.dmm_view.label = self.t_label();
        let now = self.now;
        let rel_was = self.meter.rel_active();
        let u = self.meter.update(&raw, now);
        if rel_was && !self.meter.rel_active() {
            // REL ended by the reading (an AUTO coupling switch): release
            // the range hold it took, as rel_toggle does (device check
            // 33b44bd: the range stayed MANU after the switch).
            self.dmm.hold_range(false);
        }
        // CSV recording: the unsmoothed reading (REL applied; csvlog.rs
        // header), one row per reading or per interval (the Record page's
        // setting, csvlog.rs); a function change ends it.
        if crate::csvlog::active() {
            let l = &u.raw;
            if crate::meter::func_of(l) != self.rec_func {
                crate::csvlog::stop();
            } else {
                use crate::csvlog::{F_HI, F_HOLD, F_LO, F_OL, F_REL};
                let mut f = 0u8;
                if l.main.ol {
                    f |= F_OL;
                }
                if self.meter.rel_active() {
                    f |= F_REL;
                }
                if self.meter.hold_state() != 0 {
                    f |= F_HOLD;
                }
                match self.meter.verdict() {
                    crate::meter::V_HI => f |= F_HI,
                    crate::meter::V_LO => f |= F_LO,
                    _ => {}
                }
                // The range's full scale in SI (on-change mode's threshold).
                let fs = crate::bargraph::mode_full_scale(l.mode, l.range, l.main.dec, l.main.unit) * crate::meter::unit_scale(l.main.unit);
                crate::csvlog::push(now, l.si, f, fs);
            }
        }
        let dmm_view = can_draw && !self.redraw && matches!(self.view, View::Dmm | View::DmmScope);
        // HV warning on the live value: it must not be frozen by AutoHold.
        // Strip: badge from 30 V (the stock red-value threshold), red band
        // from 100 V (the stock lightning threshold); V functions only.
        // Unsmoothed (FR-05): the warning must not lag.
        let l = &u.raw;
        // Power meter: the voltage channel.
        let v = match l.power {
            Some(p) if !p.v_ol => p.v,
            _ if l.mode <= 3 && !l.main.ol => l.si.abs(),
            _ => 0.0,
        };
        let hv_level = if v >= 100.0 { 2 } else if v >= 30.0 { 1 } else { 0 };
        let hv_changed = hv_level != self.hv_level;
        self.hv_level = hv_level;
        // DMM page: the stock >= 30 V red value (ui-review #13).
        self.dmm_view.hv_red = hv_level >= 1;
        if dmm_view && (u.show.is_some() || hv_changed || had_prov) {
            if self.pw_rng_key != 0 {
                // The first power reading after the "I ranging" texts (the
                // combined view: after its strip placeholder, #108).
                if self.view == View::Dmm {
                    ui::pw::ranging_clear();
                    self.dmm_view.invalidate();
                }
                self.pw_rng_key = 0;
            }
            self.draw_reading();
            self.hold_shown = self.shown_state();
        }
        if dmm_view && self.view == View::Dmm {
            // UF-30: the bar moves with every conversion (raw, pre-REL; also
            // while HOLD / AutoHold freeze the digits).
            crate::prof::enter(crate::prof::DBAR);
            self.dmm_view.bar_update(&raw);
            crate::prof::exit(crate::prof::DBAR);
        }
        if dmm_view {
            let hv = self.view == View::Dmm && l.mode <= 1 && !l.main.ol && l.si.abs() >= 100.0;
            if hv != self.hv_shown {
                self.hv_shown = hv;
                ui::hv_warning(hv);
                // Its box (0, 80, 40, 64) may hold the power meter's
                // "@load" mark in the parts layout: redrawn with the next
                // reading.
                self.dmm_view.pwc.coup_invalidate();
            }
        }
        self.alert(now, &raw);
        // AutoHold capture: short beep; a capture that fails the limits (with
        // the fail beep on) gets the long fail beep instead. A live value
        // crossing out of the limits: long beep, at most once a second.
        let fail = matches!(self.meter.verdict(), crate::meter::V_HI | crate::meter::V_LO);
        let fail_beep = fail && self.meter.lim_beep;
        if u.captured {
            self.beep(now, if fail_beep { 400 } else { 80 });
        } else if u.fail_edge && fail_beep && now >= self.fail_beep_at + 1000 {
            self.fail_beep_at = now;
            self.beep(now, 400);
        }
        Some(u.live)
    }

    /// Poll the scope; draws the frame in scope views.
    pub fn scope_poll(&mut self, now: u64, can_draw: bool) {
        let cyc = || unsafe { core::ptr::read_volatile(0xE000_1004 as *const u32) };
        // A scope CSV that finished (UF-13): release the record, toast.
        // The decoder's frame log owns the input (app_log.rs): no frames.
        if self.scope.log_on() {
            self.log_poll(now);
            return;
        }
        self.sp_csv_poll(now);
        // The protocol decoder: a slice of a record decode (app_decode.rs).
        self.dec_step();
        // Drag / pinch on the scope page: no new frames and no acquisition
        // restart while the finger moves the shown frame (scope_page.rs);
        // only the page diff repaint.
        let page = self.sp_on() && can_draw && !self.redraw && self.overlay.is_none();
        let covered = self.sp.covered();
        crate::scope_ui::set_flush_short(self.view == View::DmmScope);
        // The DMM CH2's stream into its store (also during a drag).
        self.ch2_drain();
        if self.sp.frozen_or_pinch() {
            if page {
                // The drag redraw (DBG:PROF? "s.frame"; touch->frame latency
                // when the shown frame moved).
                crate::prof::enter(crate::prof::FRAME);
                self.sp_tick(now);
                crate::prof::exit(crate::prof::FRAME);
                if crate::scope_ui::graph().px > 0 {
                    crate::prof::drawn();
                }
            }
            return;
        }
        // A stopped scope renders its stored record once per change (a zoom,
        // a pan, a V/div step). While nothing may be drawn (a GRAM read-back
        // or an on-device screenshot holds the panel, a host log stream, a
        // pending view redraw, an overlay on the scope page) leave that
        // render pending: polled now, the frame would be dropped and the
        // screen would keep the old trace or the drag preview (device
        // 2026-10-26).
        if !self.scope.s.run && (!can_draw || self.redraw || (self.sp_on() && self.overlay.is_some())) {
            return;
        }
        let t0 = cyc();
        crate::prof::enter(crate::prof::ACQ);
        let polled = if self.scope.wave_on() { self.wave_poll(now) } else { self.scope.poll(now) };
        crate::prof::exit(crate::prof::ACQ);
        self.prof[0] = self.prof[0].max(cyc().wrapping_sub(t0) / 200);
        if let Some(f) = polled {
            let n = f.data.len();
            // The screen columns for WAV:DATA? (scope_wav.rs, SRAM); roll
            // (#128): the middle of each column's min / max, 0 where the
            // roll has no data yet.
            for (i, w) in crate::scope_wav::screen().iter_mut().enumerate().take(if n > 0 { 400 } else { 0 }) {
                let a = i * n / 400;
                let b = ((i + 1) * n / 400).max(a + 1).min(n);
                let sum: u32 = (a..b).map(|i| f.data.at(i) as u32).sum();
                *w = (sum / (b - a) as u32) as u16;
            }
            if let (0, Some(cols)) = (n, f.cols) {
                for (w, &(lo, hi)) in crate::scope_wav::screen().iter_mut().zip(cols.iter()) {
                    *w = if lo == u16::MAX { 0 } else { (lo + hi) / 2 };
                }
                // No linear record (rolling; a stopped record zoomed out beyond
                // its length): no measurements, not the last frame's (#128).
                if !self.meas_res.v[19].is_nan() {
                    self.meas_res = crate::scope_meas::Results { v: [f32::NAN; 20] };
                    self.cursor_values();
                    self.meas = [f32::NAN; 4];
                    self.sp.cells_due = true;
                }
            }
            if self.sp.autoset.is_some() {
                self.autoset_step(&f, now);
                return;
            }
            // A stopped scope only emits a frame on a change (a zoom, a V/div
            // rescale): always measure those, they do not repeat.
            let fresh = (now >= self.meas_next || !self.scope.s.run) && !f.data.is_empty();
            if fresh {
                self.meas_next = now + 300;
                let t1 = cyc();
                crate::prof::enter(crate::prof::MEAS);
                self.meas_res = crate::scope_meas::compute(&f.data, f.fs as f32, &self.scope);
                self.cursor_values();
                crate::prof::exit(crate::prof::MEAS);
                self.prof[1] = self.prof[1].max(cyc().wrapping_sub(t1) / 200);
                let r = &self.meas_res.v;
                self.meas = [r[10], r[19], r[11], r[0]];
            }
            if !can_draw || self.redraw {
                // A one-shot capture (SINGle, DIGitize) that ended with this
                // frame stopped the scope: render its stored record on the
                // first pass that may draw, as a stopped zoom (above).
                if !self.scope.s.run {
                    self.scope.refresh();
                }
                return;
            }
            if page && covered {
                // A popup covers the page: keep the frame for later, no drawing.
                self.sp_frame(&f, now);
                self.sp_tick(now);
                return;
            }
            if page && self.sp_throttled(now) {
                // Combined view, frame not drawn: the last one's paint goes
                // on; fresh results still reach the cells.
                if fresh {
                    self.sp.cells_due = true;
                }
                self.sp_paint_pass(now, covered);
                return;
            }
            if page {
                let t2 = cyc();
                crate::prof::enter(crate::prof::FRAME);
                self.sp_frame(&f, now);
                // The screen update (the graph diff) waits for the next
                // pass's sp_tick (below): no pass does maths or frame +
                // paint (device check 2026-10-21), and USB is served
                // between the frame and its paint (f80524f: a frame pass
                // with paint was ~6 ms at ~50 fps, ping p50 11.8 ms). Frames
                // in back-to-back passes (roll, fast timebases) paint here,
                // as before, so the screen keeps up.
                if self.sp.paint_due && !fresh {
                    self.sp_tick(now);
                }
                self.sp.paint_due = true;
                crate::prof::exit(crate::prof::FRAME);
                crate::prof::drawn();
                self.prof[2] = self.prof[2].max(cyc().wrapping_sub(t2) / 200);
                // The cells follow on the next passes, one changed cell
                // each (device check 2026-10-21: measurement + frame + four
                // cells in one pass was ~32 ms).
                if fresh {
                    self.sp.cells_due = true;
                }
                return;
            }
        } else if page {
            self.sp_paint_pass(now, covered);
        }
    }

    /// A scope pass without a new frame to build: the paint of the last one
    /// (a slice, scope_ui FLUSH_BUDGET), then one changed measurement cell.
    fn sp_paint_pass(&mut self, now: u64, covered: bool) {
        self.wave_keep();
        let t0 = crate::prof::cyc();
        crate::prof::enter(crate::prof::PAINT);
        self.sp_tick(now);
        self.sp.paint_due = false;
        crate::prof::exit(crate::prof::PAINT);
        // A long paint (a status change repaints the status badge and the
        // record bar, ~14 ms on the device model): the cell waits for the
        // next pass, so no pass stacks paint + cell (~10 ms) on top of the
        // acquisition (the wave view's ring holds 26.7 ms at 76 800 S/s).
        if self.sp.cells_due && !covered && crate::prof::cyc().wrapping_sub(t0) < CELLS_AFTER_PAINT_CYC {
            self.wave_keep();
            crate::prof::enter(crate::prof::MTXT);
            self.sp_cells_step();
            crate::prof::exit(crate::prof::MTXT);
        }
    }

    /// Combined view: a new frame within COMBO_FRAME_MS of the last painted
    /// one is not drawn (true). Device check c50425f, open input, 1 ms/div:
    /// a noise-triggered trace at 56.6 fps kept the loop painting in
    /// ~1.3 ms slices on almost every pass (s.paint 63 % CPU, ~11 ms of paint
    /// per frame), DBG:PING? p50 3.3 ms against the <= 2 ms target. The meter
    /// is the main thing there; 25 fps of the scope band is plenty. A
    /// running Auto / Normal record only: Single, STOP (frames on a change),
    /// roll (partial frames), the wave view and the Scope page are never
    /// throttled.
    fn sp_throttled(&mut self, now: u64) -> bool {
        let s = &self.scope.s;
        if self.view != View::DmmScope || !s.run || s.mode == scope::TrigMode::Single || self.scope.rolling() || self.scope.wave_on() {
            return false;
        }
        if now < self.sp.frame_next {
            return true;
        }
        self.sp.frame_next = now + COMBO_FRAME_MS;
        false
    }

    /// A stopped scope emits no frames on its own: ask it for one re-render
    /// of the stored record (settings / cursor changes, view redraw).
    fn scope_invalidate(&mut self) {
        self.scope.refresh();
    }

    /// Apply scope settings (hardware + acquisition restart); the page
    /// model picks the new values up on its next update.
    fn scope_set(&mut self, s: scope::Settings, now: u64) {
        self.scope.apply(s, now);
        self.scope_invalidate();
        self.sp_settings_changed();
    }

    fn gen_set(&mut self, s: gen::Settings) {
        self.gen.apply(s);
        if self.view == View::Gen {
            ui::gen_values(&self.gen);
        }
    }

    /// Handle a gesture. Returns true when the app takes the touch (a
    /// touch-down or drag in the scope graph): the caller then tells the
    /// tracker (`gesture::Tracker::capture`), so its release is never a view
    /// swipe or a tap (../docs/history/scope-ui-plan.md §2.6).
    pub fn gesture(&mut self, clk: &mut Clock, g: Gesture) -> bool {
        let now = clk.now_ms();
        self.last_input = now;
        // Danger alarm (doc 4.10): silenced for this event by a tap on the
        // alert's badge (that tap is the alarm's: nothing else happens) or
        // toast (which closes, as any toast tap), or by any side key (which
        // still does its job: stock keys always act). Before the lock /
        // dim / screenshot gates: silencing is always allowed.
        if self.alarm.sounding() {
            let k = self.alarm_hit(&g);
            if k != 0 && self.alarm.silence() && k == 2 {
                return false;
            }
        }
        // A scope-page drag / pinch whose end would be swallowed below (a
        // screenshot, the lock, an overlay): end it now, so the acquisition
        // is not left held and no stale pinch survives (review 7389e8a B1).
        let blocked = crate::screenshot::frozen() || self.locked || self.overlay.is_some();
        if blocked && matches!(g, Gesture::PinchEnd(_) | Gesture::Up(_) | Gesture::Cancel | Gesture::KeyDown(_)) {
            self.sp_end_hold(now);
        }
        if crate::screenshot::frozen() && !matches!(g, Gesture::KeyUp(_)) {
            return false;
        }
        if self.locked {
            return false;
        }
        if self.dimmed {
            self.dimmed = false;
            backlight(self.backlight);
            // In progress: a touch-down / key-down (or a touch that began
            // before the dim) whose release is still to come. A finished
            // event (DBG:TAP / DBG:SWIPE inject a bare Tap / swipe) leaves
            // only the time window.
            let open = !matches!(
                g,
                Gesture::Tap(_) | Gesture::Up(_) | Gesture::SwipeLeft | Gesture::SwipeRight | Gesture::SwipeUp | Gesture::SwipeDown
                    | Gesture::KeyUp(_) | Gesture::MultiSwipeDown(_) | Gesture::PinchEnd(_) | Gesture::Cancel
            );
            self.wake_gate = Some((now, open));
        }
        // Wake gate: the waking touch / key and anything within
        // WAKE_GATE_MS of it only woke the screen; the idle timers restarted
        // (last_input above), as stock.
        if let Some((t, open)) = self.wake_gate {
            let start = matches!(g, Gesture::Down(_) | Gesture::KeyDown(_));
            if now < t + WAKE_GATE_MS || (open && !start) {
                return false;
            }
            self.wake_gate = None;
        }
        // A toast is not modal (owner's decision after device check f6c6e05:
        // the "AC voltage here: use AC V?" hint took the first V tap): a touch
        // on its box is for the toast (a tap there dismisses it); any other
        // touch, swipe or side key dismisses it and goes on to the page.
        if matches!(self.overlay, Some(crate::dialog::Overlay::Toast { .. })) {
            let at = match g {
                Gesture::Down(p) | Gesture::Tap(p) | Gesture::LongPress(p) | Gesture::Grab(p) | Gesture::Up(p) => Some(p),
                Gesture::Drag(s, _, _) | Gesture::Move(s, _) => Some(s),
                _ => None,
            };
            if at.is_some_and(|p| crate::dialog::toast_hit(p.x, p.y)) {
                if matches!(g, Gesture::Tap(_)) {
                    self.overlay_close();
                }
                return false;
            }
            self.overlay_close();
        }
        // A modal overlay takes every gesture: the confirm's buttons, any side
        // key cancels it.
        if let Some(o) = self.overlay {
            match (o, g) {
                (crate::dialog::Overlay::PowerOff { .. }, Gesture::Tap(p)) => match crate::dialog::power_off_hit(p.x, p.y) {
                    Some(true) => {
                        backlight(0);
                        self.persist_now();
                        power_off();
                    }
                    Some(false) => self.overlay_close(),
                    None => {}
                },
                (crate::dialog::Overlay::PowerEntry { .. }, Gesture::Tap(p)) => match ui::pw::entry_hit(p.x, p.y) {
                    Some(true) => self.pw_enter(clk),
                    Some(false) => self.overlay_close(),
                    None => {}
                },
                #[cfg(feature = "vdisk")]
                (crate::dialog::Overlay::FwInstall { .. }, Gesture::Tap(p)) => {
                    if let Some(yes) = crate::dialog::power_off_hit(p.x, p.y) {
                        crate::vwrite::answer(yes);
                        self.overlay_close();
                        if yes {
                            self.toast(now, "Installing firmware...", true);
                        }
                    }
                }
                #[cfg(feature = "vdisk")]
                (crate::dialog::Overlay::FwInstall { .. }, Gesture::KeyDown(_)) => {
                    crate::vwrite::answer(false);
                    self.overlay_close();
                }
                (crate::dialog::Overlay::EnergyReset { .. }, Gesture::Tap(p)) => match crate::dialog::power_off_hit(p.x, p.y) {
                    Some(true) => {
                        self.overlay_close();
                        self.dmm.pw.energy_reset();
                        self.beep(now, 300);
                        self.chart_dirty = true;
                    }
                    Some(false) => self.overlay_close(),
                    None => {}
                },
                (crate::dialog::Overlay::FuseCheck { stage, .. }, Gesture::Tap(p)) => match (stage, crate::dialog::fuse_hit(p.x, p.y)) {
                    // Start / Again.
                    (0, Some(true)) | (2, Some(false)) => {
                        self.dmm.fuse_start(clk);
                        self.fuse_dialog(now, 1);
                    }
                    (1, Some(false)) => {
                        self.dmm.fuse_cancel(clk);
                        self.func_changed();
                        self.overlay_close();
                    }
                    (0, Some(false)) | (2, Some(true)) => self.overlay_close(),
                    _ => {}
                },
                // Zero... / V cal... (Power Settings): Start runs the job.
                (crate::dialog::Overlay::PowJob { job, .. }, Gesture::Tap(p)) => match crate::dialog::fuse_hit(p.x, p.y) {
                    Some(true) => {
                        self.overlay_close();
                        if job == crate::dmm::pw::JOB_ZERO {
                            self.dmm.pw_zero_start(clk);
                        } else {
                            self.dmm.pw_vcal_start(clk);
                        }
                    }
                    Some(false) => self.overlay_close(),
                    None => {}
                },
                (crate::dialog::Overlay::FuseCheck { .. }, Gesture::KeyDown(_)) => {
                    self.dmm.fuse_cancel(clk);
                    self.func_changed();
                    self.overlay_close();
                }
                (crate::dialog::Overlay::PowerOff { .. } | crate::dialog::Overlay::PowerEntry { .. } | crate::dialog::Overlay::EnergyReset { .. } | crate::dialog::Overlay::PowJob { .. }, Gesture::KeyDown(_)) => {
                    self.overlay_close()
                }
                _ => {}
            }
            return false;
        }
        // The touch test page shows touches and keys, it does not act on
        // them (app_touchtest.rs).
        if self.view == View::TouchTest {
            return self.tt_gesture(g);
        }
        if self.view == View::Decode {
            if let Some(c) = self.log_gesture(clk, g) {
                return c;
            }
        }
        // The stock-replica scope page owns its touches and keys.
        if self.sp_on() {
            if let Some(c) = self.sp_gesture(clk, g) {
                return c;
            }
        }
        let mut captured = false;
        // Temperature page pressed feedback (the keypad's rule below).
        if self.temp_page_on() {
            let k = self.dmm.t_sensor();
            match g {
                Gesture::Down(p) => {
                    if let Some(c) = self.tp_pressed.take() {
                        self.with_temp_page(Some((c, false)));
                    }
                    self.tp_pressed = ui::temp_hit(k, p.x, p.y);
                    if let Some(c) = self.tp_pressed {
                        self.with_temp_page(Some((c, true)));
                    }
                }
                Gesture::Drag(_, p, _) | Gesture::Move(_, p) if ui::temp_hit(k, p.x, p.y) == self.tp_pressed => {}
                Gesture::Grab(_) => {}
                _ => {
                    if let Some(c) = self.tp_pressed.take() {
                        self.with_temp_page(Some((c, false)));
                    }
                }
            }
        }
        // Keypad pressed-key feedback (ui-review #11): the key under the
        // finger is drawn pressed from touch-down until the touch ends.
        if self.view == View::Keypad {
            match g {
                Gesture::Down(p) => {
                    if let Some(c) = self.kp_pressed.take() {
                        ui::keypad_key(self.kp_kind(), c, false);
                    }
                    self.kp_pressed = ui::keypad_hit(self.kp_kind(), p.x, p.y);
                    if let Some(c) = self.kp_pressed {
                        ui::keypad_key(self.kp_kind(), c, true);
                    }
                }
                // A finger rolled off the key (its release may emit
                // nothing): unpress; still on it: keep.
                Gesture::Drag(_, p, _) | Gesture::Move(_, p) if ui::keypad_hit(self.kp_kind(), p.x, p.y) == self.kp_pressed => {}
                Gesture::Grab(_) => {}
                _ => {
                    if let Some(c) = self.kp_pressed.take() {
                        ui::keypad_key(self.kp_kind(), c, false);
                    }
                }
            }
        }
        // The same rule on the menu tiles, list-page rows / stepper keys and
        // the generator boxes (app_press.rs).
        self.press_gesture(&g);
        match g {
            // Global gestures (UF-17 / UF-18), any view.
            Gesture::MultiSwipeDown(3) => self.shot_request(now),
            // One finger down the DMM page: start / stop the CSV recording.
            Gesture::SwipeDown if self.view == View::Dmm => self.rec_toggle(now),
            Gesture::MultiSwipeDown(2) => {
                self.overlay = Some(crate::dialog::Overlay::PowerOff { until: now + crate::dialog::CONFIRM_MS });
                self.redraw = true;
                crate::dialog::power_off();
                self.beep(now, 50);
            }
            Gesture::KeyDown(c) => {
                self.beep(now, 50);
                self.key(clk, c);
            }
            Gesture::KeyUp(1) if self.run_key == 1 => self.run_down = None,
            // AUTO released before RUN_LONG_MS: the short press.
            Gesture::KeyUp(2) if self.run_key == 2 && self.run_down.take().is_some() => self.auto_key_end(clk, false),
            Gesture::Tap(_) | Gesture::SwipeLeft | Gesture::SwipeRight if self.view == View::Gallery => {
                if matches!(g, Gesture::Tap(_)) {
                    self.beep(now, 30);
                }
                if crate::gallery::get().touch(g) == crate::gallery::Out::Exit {
                    self.show(View::Menu, clk);
                }
            }
            Gesture::SwipeLeft | Gesture::SwipeRight if !is_page(self.view) => {
                let i = View::CYCLE.iter().position(|&v| v == self.view).unwrap_or(0);
                let n = View::CYCLE.len();
                let j = if g == Gesture::SwipeLeft { (i + 1) % n } else { (i + n - 1) % n };
                self.show(View::CYCLE[j], clk);
            }
            Gesture::Tap(p) => {
                self.beep(now, 30);
                self.tap(clk, p.x, p.y);
                // Pressed feedback kept through the tap (app_press.rs).
                if self.pressed.is_some() {
                    self.press_after_tap();
                }
            }
            // Long press on the A button: the power meter's wiring dialog.
            Gesture::LongPress(p) if self.view == View::Dmm && p.y >= ui::BTN_Y && ui::button_hit(p.x, p.y) == Some(1) => {
                captured = true;
                self.pw_open_entry(now);
            }
            // Power meter: a long press on the energy readout (aux2) or the
            // side column asks to reset the energy.
            Gesture::LongPress(p) if self.view == View::Dmm && self.dmm.pw.on && ((p.x >= 402 && (175..279).contains(&p.y)) || ui::pw::energy_hit(p.x, p.y)) => {
                captured = true;
                self.pw_long_side(now);
            }
            Gesture::LongPress(p) if self.view == View::Dmm && self.dmm.pw.on && (153..172).contains(&p.y) => captured = true,
            Gesture::LongPress(p) if self.view == View::Dmm && !self.dmm_view.bar_on && (153..172).contains(&p.y) => {
                captured = true;
                self.hist_n = 0;
                self.beep(now, 300);
                ui::history(&self.hist, 0, 0);
            }
            Gesture::LongPress(p) if self.view == View::Dmm && p.x >= 402 && (175..279).contains(&p.y) => {
                captured = true;
                // MIN/MAX/AVG column (REL box included): reset the statistics
                // (UF-35; distinct from the history-row clear above).
                self.meter.stats.clear();
                self.beep(now, 300);
                self.chart_dirty = true;
            }
            _ => {}
        }
        captured
    }

    /// A gesture that silences the danger alarm: 0 no, 1 yes and it goes on
    /// (a side key, a tap on a toast), 2 yes and it ends here (a tap on the
    /// alert badge: the DMM status slot ui::hold_label 119..202 x 1..33,
    /// the combined view's strip badge, scope_ui ST_BADGE 19..81 x 46..63;
    /// a few px around both).
    fn alarm_hit(&self, g: &Gesture) -> u8 {
        let p = match *g {
            Gesture::KeyDown(_) => return 1,
            Gesture::Tap(p) => p,
            _ => return 0,
        };
        let (x, y) = (p.x, p.y);
        if matches!(self.overlay, Some(crate::dialog::Overlay::Toast { .. })) && crate::dialog::toast_hit(x, y) {
            return 1;
        }
        let badge = match self.view {
            View::Dmm => (115..206).contains(&x) && y < 37,
            View::DmmScope => (15..85).contains(&x) && (42..67).contains(&y),
            _ => false,
        };
        if badge { 2 } else { 0 }
    }

    /// Close the overlay; the view is redrawn on the next tick.
    fn overlay_close(&mut self) {
        self.overlay = None;
        self.redraw = true;
        crate::dialog::toast_gone();
    }

    /// A screenshot gesture (3-finger swipe, the scope's Print screen). A
    /// fast-log session or an upload holds the disk for an unbounded time:
    /// "Disk busy" now, nothing written. The request used to wait in
    /// `shot_req` for `can_draw`, which a fast-log session keeps false, so it
    /// ran after the session (device 2026-10-23, S023) and never got the
    /// Busy answer of screenshot.rs.
    pub(crate) fn shot_request(&mut self, now: u64) {
        if crate::fastlog::holds_disk() || crate::upload::busy() {
            self.shot_toast(now, crate::screenshot::Shot::Busy);
        } else {
            self.shot_req = true;
        }
    }

    /// UF-17: start writing the screen to the disk (screenshot.rs, in slices
    /// from tick(), ~5 s; the screen is frozen until its last row is read):
    /// a chirp now, a toast + beep when done (`shot_toast`). A swipe while one
    /// is in flight is ignored with a low beep.
    fn screenshot(&mut self, now: u64) {
        let ok = crate::screenshot::start();
        self.beep(now, if ok { 60 } else { 300 });
    }

    /// UF-28: before the idle power-off, "Auto power-off in 30 s" (20, 10)
    /// with a beep that ignores a 0 % volume. Not an input: the countdown
    /// goes on unless the user touches the screen or a key.
    fn off_warn(&mut self, now: u64, idle: u64) {
        let off = self.off_ms();
        if off == 0 || self.meter.stats.rec || self.dmm.pw.integrating() || crate::csvlog::active() || self.scope.log_on() || idle + OFF_WARN_MS <= off {

            self.off_warned = 0;
            return;
        }
        let k = (idle + OFF_WARN_MS - off) / OFF_WARN_STEP_MS;
        if k < self.off_warned || k >= OFF_WARN_MS / OFF_WARN_STEP_MS {
            return;
        }
        self.off_warned = k + 1;
        // Dimmed: full backlight for the countdown (`dimmed` stays set, so
        // the idle dim does not re-trigger; any input restores as usual).
        if self.dimmed {
            backlight(self.backlight);
        }
        buzzer(true, self.volume.max(ALERT_VOL));
        self.beep_until = now + 200;
        let mut b = [0u8; 32];
        let mut t = [0u8; 16];
        let secs = crate::dmm::format(&mut t, ((OFF_WARN_MS - k * OFF_WARN_STEP_MS) / 1000) as f32, 0);
        let mut n = 0;
        for part in ["Auto power-off in ", secs, " s"] {
            b[n..n + part.len()].copy_from_slice(part.as_bytes());
            n += part.len();
        }
        let txt = core::str::from_utf8(&b[..n]).unwrap_or("");
        if !self.toast_defer(txt, false) {
            self.toast_draw(now, txt, false);
        }
    }

    /// Wrong-connection alert of the latest reading (sentinel.rs): a toast
    /// once per assertion, in any view; LIVE? beeps once 100 ms, V-JACK? is
    /// silent. The badge comes with the reading (DMM status slot,
    /// combined-view strip). The dangerous ones (doc 4.10: A-JACK, A-PROT
    /// while its current flows, a current above the 10 A rating) are the
    /// danger alarm's (`alert_beep`): noted here per reading.
    fn alert(&mut self, now: u64, r: &Reading) {
        use crate::sentinel::Alert;
        let a = r.alert;
        // A-PROT: dangerous while the current flows. It trips only in DC V /
        // mV, which keep the probe in the row while latched (doc 4.8): its
        // reading below APROT_GONE_A = the source is gone and the latch only
        // waits for the user (badge, no alarm). In a function without the
        // probe (the latch holds there too) its last value stands: a source
        // removed before the change stays quiet (the user did the right
        // thing: no new alarm), one still there keeps sounding (the cap).
        // A-JACK only where V-COM cannot fake it (sentinel `ajack_alarm`:
        // not in the ohm-network rows; doc 4.10): the badge stays.
        self.danger_rd = (a == Alert::AJack && self.dmm.sent.ajack_alarm()) || (a == Alert::AProt && self.dmm.sent.probe_a >= crate::sentinel::APROT_GONE_A);
        let pw = r.power.map(|p| (p.ik, p.i_ol, p.i));
        if crate::danger::over_rating(r.mode, r.range, r.main.ol, r.si, pw) {
            self.i10_since.get_or_insert(now);
        } else {
            self.i10_since = None;
        }
        let seq = self.dmm.sent.seq;
        if seq != self.alert_seq {
            self.alert_seq = seq;
            if a != Alert::None {
                if !is_page(self.view) || self.view == View::Menu {
                    self.toast(now, crate::dmm::pw::hint_toast(a, self.dmm.pw.on), false);
                }
                match a {
                    Alert::Live => self.beep(now, 100),
                    // OVERLOAD: one 300 ms beep (doc 4.4) at the key / alert
                    // volume also with the key beep off (a safety cue, as
                    // A-JACK's pattern); the toast says why the range changed.
                    Alert::Overload => {
                        buzzer(true, self.volume);
                        self.beep_until = now + 300;
                    }
                    _ => {}
                }
            }
        }
        // The power meter's expired current limit (its own pattern).
        if !(self.dmm.pw.on && self.dmm.pw_state().over == 2) {
            self.alert_beep_t0 = None;
        }
    }

    /// Per pass: the danger alarm (danger.rs, doc 4.10) owns the buzzer
    /// while it sounds (Loud: 100 % whatever the volume; Volume: the key /
    /// alert volume). Only with the DMM running: no readings, no claim (the
    /// scope / generator views switch its rail off). Otherwise the power
    /// meter's current-limit pattern: 3 x 50 ms (50 ms apart) every 2 s at
    /// the key / alert volume (0 % = silent); never over another beep in
    /// progress.
    fn alert_beep(&mut self, now: u64) {
        let i10 = self.i10_since.is_some_and(|t| now.saturating_sub(t) >= crate::danger::I10_ASSERT_MS);
        let active = (self.danger_rd || i10) && self.scope.dmm_rail();
        match self.alarm.feed(now, active, self.alarm_mode != crate::danger::OFF) {
            crate::danger::Out::Tone(t) => {
                buzzer_tone(t, if self.alarm_mode == crate::danger::LOUD { 100 } else { self.volume });
                self.alert_buzz = false;
                return;
            }
            crate::danger::Out::Stop => {
                buzzer_tone(0, 0);
                self.beep_until = 0;
                self.alert_buzz = false;
            }
            crate::danger::Out::Idle => {}
        }
        let on = match self.alert_beep_t0 {
            Some(t0) => {
                let ph = now.saturating_sub(t0) % 2000;
                ph < 250 && (ph / 50) % 2 == 0
            }
            None => false,
        };
        if on != self.alert_buzz && self.beep_until == 0 {
            self.alert_buzz = on;
            buzzer(on, self.volume);
        }
    }

    /// Fuse-check dialog (doc 4.3 / 4.7) at `stage` (0 instructions, 1
    /// measuring, 2 the verdict of `Dmm::fuse_state`).
    fn fuse_dialog(&mut self, now: u64, stage: u8) {
        use crate::dialog as d;
        let until = now + if stage == 1 { 20_000 } else { 60_000 };
        self.overlay = Some(d::Overlay::FuseCheck { until, stage });
        self.redraw = true;
        self.last_input = now;
        let mut b = [0u8; 32];
        ui::fuse_screen(stage, self.dmm.fuse_state(), &mut b);
    }

    /// Per tick: a fuse check that ended (dialog or SYST:FUSE:TEST) -> the
    /// meter restarts on the function it went back to; the dialog shows
    /// the verdict with a beep (100 ms OK, 300 ms otherwise).
    fn fuse_tick(&mut self, _clk: &mut Clock, now: u64) {
        let busy = self.dmm.fuse_state() == crate::sentinel::FuseRes::Busy;
        if self.fuse_busy && !busy {
            self.func_changed();
            let aborted = self.dmm.fuse_state() == crate::sentinel::FuseRes::Idle;
            if aborted && matches!(self.overlay, Some(crate::dialog::Overlay::FuseCheck { .. })) {
                // Ended by another choice (a SCPI CONF:, the BT mode byte).
                self.overlay_close();
            } else if let Some(crate::dialog::Overlay::FuseCheck { stage: 1, .. }) = self.overlay {
                let ok = matches!(self.dmm.fuse_state(), crate::sentinel::FuseRes::Ok(_));
                buzzer(true, self.volume);
                self.beep_until = now + if ok { 100 } else { 300 };
                self.fuse_dialog(now, 2);
            }
        }
        self.fuse_busy = busy;
    }

    /// A toast over the view (dialog.rs); the view is redrawn when it goes.
    /// Dropped while a screenshot reads the panel (it would land in the rows
    /// not read yet; screenshot.rs "Frozen image"); the caller's beep stays.
    /// A confirm / dialog up (`Overlay::modal`): the toast waits for its end
    /// (`toast_defer`) instead of replacing it. A toast used to take the
    /// overlay slot: the confirm vanished without its No path (the energy
    /// reset's Yes then hit the bare page: powtest ereset-hint; a fuse check
    /// kept measuring, a firmware question stayed unanswered).
    fn toast(&mut self, now: u64, txt: &str, ok: bool) {
        self.last_input = now;
        if crate::screenshot::frozen() || self.toast_defer(txt, ok) {
            return;
        }
        self.toast_draw(now, txt, ok);
    }

    /// Keep `txt` for after the confirm that is up (or the scope popup's
    /// repaint, `toast_hold`); false = none is up.
    fn toast_defer(&mut self, txt: &str, ok: bool) -> bool {
        if !self.toast_hold && !self.overlay.is_some_and(|o| o.modal()) {
            return false;
        }
        let mut n = txt.len().min(TOAST_LATER_MAX);
        while !txt.is_char_boundary(n) {
            n -= 1;
        }
        self.toast_later[..n].copy_from_slice(&txt.as_bytes()[..n]);
        self.toast_later_n = n as u8;
        self.toast_later_ok = ok;
        true
    }

    /// The deferred toast, once the confirm has gone and the page under it
    /// is drawn again (`tick`). Not an input: the idle timers go on.
    #[inline(never)]
    fn toast_later_show(&mut self, now: u64) {
        let (b, n) = (self.toast_later, self.toast_later_n as usize);
        self.toast_later_n = 0;
        if crate::screenshot::frozen() {
            return;
        }
        self.toast_draw(now, core::str::from_utf8(&b[..n]).unwrap_or(""), self.toast_later_ok);
    }

    fn toast_draw(&mut self, now: u64, txt: &str, ok: bool) {
        self.sp_end_hold(now);
        self.overlay = Some(crate::dialog::Overlay::Toast { until: now + crate::dialog::TOAST_MS });
        self.redraw = true;
        crate::dialog::toast(txt, ok);
    }

    /// Start a CSV recording of the current DMM function (DMM page swipe
    /// down, SCPI LOG:CSV ON). False: no reading of the function yet, a
    /// recording is already running.
    #[inline(never)] // .cold (link-native.x): once per recording
    pub fn rec_start(&mut self, now: u64) -> bool {
        let Some(r) = self.meter.live().copied() else { return false };
        let func = crate::meter::func_of(&r);
        let range = if r.auto { "AUTO" } else { "MANUAL" };
        if !crate::csvlog::start(crate::csvlog::func_label(func), crate::meter::base_unit(r.main.unit), range, now) {
            return false;
        }
        self.rec_func = func;
        true
    }

    /// Why a CSV recording cannot start now (SCPI LOG:CSV ON; the pages
    /// that start one are not reachable then): the Wave view and the inrush
    /// page stream AD2 and pause the DMM poll, so it would record no rows
    /// (device 9d56ca6: accepted, READ? 9.9E37; docs/ram-budget.md, the
    /// logger::RING row). Their own entries refuse a running recording.
    pub fn rec_conflict(&self) -> Option<&'static str> {
        if self.scope.wave_on() {
            Some("Settings conflict: Wave view")
        } else if self.view == View::Inrush {
            Some("Settings conflict: inrush page")
        } else {
            None
        }
    }

    /// DMM page swipe down: stop a running recording (its result toast
    /// follows when the last rows are on the disk), or start one.
    fn rec_toggle(&mut self, now: u64) {
        if crate::csvlog::active() {
            crate::csvlog::stop();
            self.beep(now, 60);
        } else if self.rec_start(now) {
            self.beep(now, 60);
            self.toast(now, "Recording CSV", true);
        } else {
            self.beep(now, 300);
            self.toast(now, "No reading to log", false);
        }
    }

    /// The outcome of a finished CSV recording.
    fn rec_toast(&mut self, now: u64, st: crate::csvlog::Status) {
        use crate::csvlog::State;
        let mut t = *b"Saved LOG000.DLG";
        let (txt, ok) = match (st.state, st.file) {
            (State::Done, Some(n)) => {
                t[9] = b'0' + (n / 100 % 10) as u8;
                t[10] = b'0' + (n / 10 % 10) as u8;
                t[11] = b'0' + (n % 10) as u8;
                (core::str::from_utf8(&t).unwrap_or("Saved"), true)
            }
            (State::Full, _) => ("Disk full: log ended", false),
            (State::NoDisk, _) => ("No disk", false),
            _ => ("Log failed", false),
        };
        self.toast(now, txt, ok);
        self.beep(now, if ok { 100 } else { 500 });
    }

    fn shot_toast(&mut self, now: u64, shot: crate::screenshot::Shot) {
        use crate::screenshot::Shot;
        self.last_input = now;
        let mut t = *b"Saved S000.BMP";
        let (txt, ok) = match shot {
            Shot::Saved(n) => {
                t[7] = b'0' + (n / 100 % 10) as u8;
                t[8] = b'0' + (n / 10 % 10) as u8;
                t[9] = b'0' + (n % 10) as u8;
                (core::str::from_utf8(&t).unwrap_or("Saved"), true)
            }
            Shot::NoDisk => ("No disk", false),
            Shot::Full => ("Disk full", false),
            Shot::Error => ("Screenshot failed", false),
            Shot::Busy => ("Disk busy", false),
        };
        self.toast(now, txt, ok);
        self.beep(now, if ok { 100 } else { 500 });
    }

    #[inline(never)] // .cold (link-native.x): on a key press
    fn key(&mut self, clk: &mut Clock, code: u8) {
        let now = clk.now_ms();
        match code {
            0 if self.band_until != 0 => self.band_close(),
            0 if self.view == View::Gallery => {
                if crate::gallery::get().back() == crate::gallery::Out::Exit {
                    self.show(View::Menu, clk);
                }
            }
            0 => {
                // Walk one step up the page tree, not straight to the
                // instrument: Settings 3/3 -> 2/3 -> 1/3 -> Menu -> instrument.
                // The old code sent every menu page to back_view, so the back
                // key in Settings skipped their parent. Device -> the main
                // menu: on the stock, Device is a section of the one Settings
                // screen, and MENU there leaves Settings (device check 519fb8a:
                // MENU on Device returned to Settings 3/3, where the next
                // "menu" tap changed the USB disk row). Range / Keypad still
                // pop to the view they were opened from.
                let v = match self.view {
                    View::Device => View::Menu,
                    View::Help => View::Settings,
                    View::Settings if self.set_page >= 1 => {
                        self.set_page -= 1;
                        View::Settings
                    }
                    View::Settings => View::Menu,
                    View::Keypad if self.kp_dmm() => self.kp_back(),
                    View::Record => self.rec_back,
                    View::Limits => self.limits_back(),
                    View::Menu | View::Range | View::Keypad | View::Inrush => self.back_view,
                    _ => View::Menu,
                };
                self.show(v, clk);
            }
            1 => match self.view {
                // The inrush page: RUN = Arm / Cancel.
                View::Inrush => self.inrush_button(clk),
                View::Gen => {
                    let mut st = self.gen.s;
                    st.on = !st.on;
                    self.gen_set(st);
                }
                // (The scope page handles its own keys: scope_page.rs.)
                _ => {
                    let h = !self.meter.hold;
                    self.meter.set_hold(h);
                    self.dmm_view.invalidate();
                    if matches!(self.view, View::Dmm | View::DmmScope) {
                        self.run_down = Some(now);
                        self.run_key = 1;
                    }
                }
            },
            // The inrush page holds the 10 A range: no autorange.
            2 if self.view == View::Inrush => self.beep(now, 300),
            // The generator's Signal page (and its keypad) has no AUTO
            // function: the key does nothing there (it used to set the
            // DMM's autorange).
            2 if self.view == View::Gen || (self.view == View::Keypad && !self.kp_dmm()) => {}
            // V / A / ohm: the range step on release, the autorange when held
            // (auto_key_end).
            2 if !self.dmm.pw.on && matches!(self.dmm.button(), Button::Volt | Button::Amp | Button::Ohm) => {
                self.run_down = Some(now);
                self.run_key = 2;
            }
            2 => {
                self.meter.rel_off();
                self.meter.set_hold(false);
                let was = self.dmm.is_auto();
                self.dmm.auto_key(clk);
                self.auto_entered(was);
                self.dmm_view.invalidate();
            }
            _ => match self.view {
                // Power meter: its Power Settings page (View::Range).
                View::Dmm | View::DmmScope if self.has_range_page() => self.show(View::Range, clk),
                // No range page (continuity / diode, frequency): the Hold &
                // Limits page.
                View::Dmm | View::DmmScope => self.show(View::Limits, clk),
                View::Range | View::Limits | View::Record | View::Inrush => {
                    let v = self.back_view;
                    self.show(v, clk)
                }
                _ => {}
            },
        }
    }

    /// AUTO in V / A / ohm, released (`long` false) or held RUN_LONG_MS.
    /// Short: in autorange the current range becomes the manual one; in a
    /// manual range the next higher one (the MF page's order, 60 mV and
    /// 60 ohm included), the highest wrapping to the lowest. Long: the
    /// default autorange (V: AUTO+, A and ohm: AUTO). Both release HOLD and
    /// REL as before; AutoHold stays as it is.
    #[inline(never)]
    fn auto_key_end(&mut self, clk: &mut Clock, long: bool) {
        let now = clk.now_ms();
        let b = self.dmm.button();
        // The function changed while the key was held: nothing.
        if self.dmm.pw.on || !matches!(b, Button::Volt | Button::Amp | Button::Ohm) {
            return;
        }
        self.meter.rel_off();
        self.meter.set_hold(false);
        let item = if long {
            self.beep(now, 200);
            if b == Button::Volt { Some(255) } else { None }
        } else {
            let items = ui::range_items(b);
            let manual = |i: usize| items.get(i).and_then(|it| it.1).filter(|&c| c != 255);
            let n = (0..items.len()).take_while(|&i| manual(i).is_some()).count();
            let cur = self.range_sel_manual();
            // The items before AUTO, in a ring; from one after it (ohm
            // 600 Mohm, experimental) back to the first.
            let i = match cur {
                Some(i) if !self.dmm.is_auto() => if i + 1 < n { i + 1 } else { 0 },
                Some(i) => i,
                None => 0,
            };
            manual(i)
        };
        // Held into AUTO: the placeholder (AUTO, dashes) at once, as the
        // range page's AUTO item and RES:RANG:AUTO do; before, the last
        // manual reading stayed with MANU until the next conversion.
        let was = self.dmm.is_auto();
        self.dmm.set_range(clk, item);
        self.auto_entered(was);
        self.dmm_view.invalidate();
        self.redraw |= matches!(self.view, View::Range | View::Limits);
    }

    #[inline(never)] // .cold (link-native.x): on a tap
    fn tap(&mut self, clk: &mut Clock, x: u16, y: u16) {
        let now = clk.now_ms();
        if self.dmm.pw.on && self.pw_tap(clk, x, y) {
            return;
        }
        match self.view {
            // REL button (stock (402,205,76,38); 36 high since SD, ui::chart).
            View::Dmm if x >= 402 && (205..241).contains(&y) => self.rel_toggle(now),
            View::Dmm if x >= 402 && (175..279).contains(&y) => {
                // MIN/MAX/AVG column outside the REL box: start / stop a
                // recording session. Starting clears the statistics; while it
                // runs the idle shutdown is off (FR-02). Long press = reset.
                let on = !self.meter.stats.rec;
                if on {
                    self.meter.stats.clear();
                }
                self.meter.stats.rec = on;
                self.chart_dirty = true;
            }
            View::Dmm if !self.dmm_view.bar_on && (153..172).contains(&y) => {
                // History row: record the shown value (not OL). While HOLD
                // (or an AutoHold capture) that is the frozen value (UF-27:
                // owners hold a reading, then store it).
                if let Some(r) = self.meter.shown().copied() {
                    if !r.main.ol {
                        let i = if self.hist_n < 5 { self.hist_n } else { (self.hist_new + 1) % 5 };
                        self.hist[i] = (r.main.v, r.main.dec, r.main.unit);
                        self.hist_n = (self.hist_n + 1).min(5);
                        self.hist_new = i;
                        ui::history(&self.hist, self.hist_n, self.hist_new);
                    }
                }
            }
            View::Gallery => {} // gestures go to gallery::touch (App::gesture)
            // Gestures go to app_touchtest.rs (App::gesture).
            View::TouchTest => {}
            View::Inrush => self.inrush_tap(clk, x, y),
            // (The frame log handles its own taps: app_log.rs.)
            View::Decode => {}
            View::Device => {
                // Any tap dismisses back to Settings (its parent; MENU goes
                // on to the main menu, see key()).
                self.show(View::Settings, clk);
            }
            View::Help => {
                // Title bar: back to Settings 2/2 (as MENU); elsewhere: the
                // next help page (wraps).
                if ui::title_back_hit(y) {
                    self.show(View::Settings, clk);
                } else {
                    self.help_page = (self.help_page + 1) % ui::HELP.len();
                    self.redraw = true;
                }
            }
            View::Keypad => {
                let freq = self.kp_target == 0;
                let back = self.kp_back();
                let key = ui::keypad_hit(self.kp_kind(), x, y);
                // Select-all prefill: the first digit / '.' / backspace
                // replaces the whole value; H/K/M keep it selected (a unit
                // change); the sign toggle edits it.
                match key {
                    Some(b'=') | None => {}
                    Some(c) => {
                        if self.kp_sel && !matches!(c, b'H' | b'K' | b'M' | b'-') {
                            self.kp_n = 0;
                        }
                        if c != b'H' && c != b'K' && c != b'M' {
                            self.kp_sel = false;
                        }
                        self.kp_edit = true;
                    }
                }
                match key {
                    Some(b'=') => {
                        if self.kp_edit {
                            self.kp_apply();
                        }
                        if core::mem::take(&mut self.ntc_reenter) {
                            self.dmm.set_t_sensor(clk, 6);
                            self.func_changed();
                            self.last_raw = None;
                        }
                        self.show(back, clk);
                    }
                    Some(b'<') => self.kp_n = self.kp_n.saturating_sub(1),
                    Some(c @ (b'H' | b'K' | b'M')) => self.kp_mult = c,
                    Some(b'-') => {
                        // Sign toggle at the front.
                        let n = self.kp_n;
                        if self.kp_buf[..n].first() == Some(&b'-') {
                            self.kp_buf.copy_within(1..n, 0);
                            self.kp_n -= 1;
                        } else if n < self.kp_buf.len() - 1 {
                            self.kp_buf.copy_within(0..n, 1);
                            self.kp_buf[0] = b'-';
                            self.kp_n += 1;
                        }
                    }
                    Some(c) => {
                        // Limits: up to 5 digits plus sign and point.
                        let digits = self.kp_buf[..self.kp_n].iter().filter(|d| d.is_ascii_digit()).count();
                        let ok = match self.kp_target {
                            0 => self.kp_n < 7,
                            1 | 2 | 5 | KP_NTC_BETA => self.kp_n < 5,
                            KP_T_OFFS => self.kp_n < 5,
                            KP_NTC_R25 => self.kp_n < 7,
                            _ => c == b'.' || digits < 5,
                        };
                        if ok && self.kp_n < self.kp_buf.len() - 1 && !(c == b'.' && self.kp_buf[..self.kp_n].contains(&b'.')) {
                            self.kp_buf[self.kp_n] = c;
                            self.kp_n += 1;
                        }
                    }
                    None => {
                        // Outside the keypad: cancel.
                        if !(x >= 224 && y >= if freq { 142 } else { 100 }) {
                            self.show(back, clk);
                        }
                        return;
                    }
                }
                let (_, unit) = self.kp_labels();
                let n = self.kp_n;
                let t = core::str::from_utf8(&self.kp_buf[..n]).unwrap_or("");
                ui::keypad_input(freq, t, unit, self.kp_sel);
            }
            View::Range if self.dmm.hy_mode() == 15 => {
                let k = self.dmm.t_sensor();
                match ui::temp_hit(k, x, y) {
                    Some(ui::TK_OK) => {
                        let v = self.back_view;
                        self.show(v, clk);
                        return;
                    }
                    Some(ui::TK_LIMITS) => {
                        self.show(View::Limits, clk);
                        return;
                    }
                    Some(ui::TK_RECORD) => {
                        self.record_open(clk);
                        return;
                    }
                    Some(i @ 0..=6) => {
                        if i != k {
                            self.dmm.set_t_sensor(clk, i);
                            self.func_changed();
                            self.last_raw = None;
                        }
                    }
                    Some(ui::TK_UNIT) => {
                        let f = !self.dmm.temp_f();
                        self.set_temp_f(f);
                    }
                    Some(i @ (ui::TK_MINUS | ui::TK_PLUS)) => {
                        let d = if i == ui::TK_MINUS { -0.1 } else { 0.1 };
                        self.set_t_offs(self.dmm.t_offs(k) + d);
                        return;
                    }
                    Some(ui::TK_OFFS) => {
                        self.kp_open(KP_T_OFFS, clk);
                        return;
                    }
                    Some(ui::TK_R25) => {
                        self.kp_open(KP_NTC_R25, clk);
                        return;
                    }
                    Some(ui::TK_BETA) => {
                        self.kp_open(KP_NTC_BETA, clk);
                        return;
                    }
                    Some(_) | None => return,
                }
                self.redraw = true;
            }
            View::Range => {
                let b = self.dmm.button();
                if app_wave::WAVE_BUILD && matches!(b, Button::Volt | Button::Amp) && ui::wave_hit(x, y) {
                    self.wave_open(clk);
                    return;
                }
                if app_wave::WAVE_BUILD && b == Button::Amp && ui::inrush_hit(x, y) {
                    let _ = self.inrush_open(clk);
                    return;
                }
                match ui::range_hit(b, self.range_sel(), x, y) {
                    Some(Ok(i)) => {
                        let item = ui::range_items(b)[i].1;
                        let was = self.dmm.is_auto();
                        self.dmm.set_range(clk, item);
                        self.auto_entered(was);
                        self.meter.rel_off();
                        self.redraw = true;
                    }
                    Some(Err(2)) => {
                        let v = self.back_view;
                        self.show(v, clk);
                    }
                    Some(Err(4)) => self.show(View::Limits, clk),
                    Some(Err(7)) => self.record_open(clk),
                    Some(Err(5)) if b == Button::Amp => self.fuse_dialog(now, 0),
                    Some(Err(k @ 0..=1)) if b == Button::Ohm => {
                        if k != self.dmm.ohm_var() {
                            self.dmm.set_ohm_var(clk, k);
                            self.func_changed();
                        }
                        self.redraw = true;
                    }
                    Some(Err(k @ (0 | 1 | 3 | 6))) => {
                        // DC / AC / AC+DC / AUTO buttons (Err 0 / 1 / 3 / 6)
                        // -> sub-mode 0 / 1 / 2 / 3.
                        let sub = match k {
                            3 => 2,
                            6 => 3,
                            k => k,
                        };
                        if sub != self.dmm.sub() {
                            self.dmm.set_sub(clk, sub);
                            self.func_changed();
                        }
                        self.redraw = true;
                    }
                    Some(Err(_)) | None => {}
                }
            }
            View::Dmm => {
                if ui::value_hit(x, y) {
                    self.resolution_cycle(now);
                } else if ui::rate_chip_hit(x, y) {
                    self.rate_cycle(now);
                } else if x < ui::CHART_X && ui::chart_rows().contains(&y) {
                    self.chart_rate = ui::trend_next(self.chart_rate);
                    self.chart_n = 0;
                    self.chart_dirty = true;
                } else if let Some(i) = ui::button_hit(x, y) {
                    self.press_func(clk, i);
                }
            }
            View::DmmScope => {
                // Open function band: its buttons (kept open 4 s from the
                // last tap); a tap anywhere else closes it first.
                if self.band_until != 0 {
                    if let Some(i) = ui::func_band_hit(x, y) {
                        self.band_until = now + BAND_MS;
                        self.press_func(clk, i);
                        return;
                    }
                    self.band_close();
                    return;
                }
                // (Strip, panel, badges: scope_page.rs.)
            }
            // The scope page handles its own taps (scope_page.rs).
            View::Scope => {}
            View::Gen => {
                let mut st = self.gen.s;
                if ui::gen_run_hit(x, y) {
                    // RUN/STOP label: output on / off, as the RUN key.
                    st.on = !st.on;
                } else if ui::gen_wave_hit(x, y) {
                    let k = Wave::ALL.iter().position(|&w| w == st.wave).unwrap_or(0);
                    st.wave = Wave::ALL[(k + 1) % 4];
                } else if let Some(b) = ui::gen_box_hit(x, y) {
                    // Frequency, Vpp, Offset, Duty (square only, any f).
                    match b {
                        0 => self.kp_open(0, clk),
                        1 => self.kp_open(1, clk),
                        2 => self.kp_open(5, clk),
                        _ if st.wave == Wave::Square => self.kp_open(2, clk),
                        _ => {}
                    }
                    return;
                } else {
                    return;
                }
                // Wave change: the set frequency is kept as is (a wave that
                // cannot play it runs at its limit, gen::Gen::freq_clamped),
                // so cycling back to Square restores it.
                self.gen_set(st);
            }
            View::Menu => match ui::main_menu_hit(x, y) {
                Some(0) => self.show(View::Dmm, clk),
                Some(1) => self.show(View::Scope, clk),
                Some(2) => self.show(View::Gen, clk),
                Some(3) => {
                    self.set_page = 0;
                    self.show(View::Settings, clk)
                }
                Some(5) => {
                    crate::gallery::get().open();
                    self.show(View::Gallery, clk);
                }
                Some(_) => {
                    let c = self.combo;
                    self.show(c, clk);
                }
                None => {}
            },
            View::Limits => {
                if ui::title_back_hit(y) {
                    let v = self.limits_back();
                    self.show(v, clk);
                    return;
                }
                let slot = crate::meter::lim_slot(self.dmm.func_shown());
                let live = self.meter.live().copied().filter(|r| !r.main.ol && crate::meter::func_of(r) == self.dmm.func_shown());
                let l = &mut self.meter.lim[slot];
                match ui::list_hit(y, ui::LIMIT_ROWS.len()) {
                    Some(0) => {
                        let on = !self.meter.ahold;
                        self.meter.set_ahold(on);
                    }
                    Some(1) => l.on = !l.on,
                    Some(k @ (2 | 3)) => {
                        self.kp_unit = self.lim_unit();
                        self.kp_open(if k == 2 { 3 } else { 4 }, clk);
                        return;
                    }
                    // Limit = the live (REL-applied) reading; also turns the
                    // check on.
                    Some(k @ (4 | 5)) => match live {
                        Some(r) => {
                            if k == 4 {
                                l.hi = Some(r.si);
                            } else {
                                l.lo = Some(r.si);
                            }
                            l.on = true;
                        }
                        None => return,
                    },
                    Some(6) => {
                        l.hi = None;
                        l.lo = None;
                    }
                    Some(7) => self.meter.lim_beep = !self.meter.lim_beep,
                    Some(8) => {
                        self.record_open(clk);
                        return;
                    }
                    _ => return,
                }
                self.meter.recheck();
                self.redraw = true;
            }
            View::Record => {
                if ui::title_back_hit(y) {
                    let v = self.rec_back;
                    self.show(v, clk);
                    return;
                }
                let now = clk.now_ms();
                match ui::list_hit(y, ui::RECORD_ROWS.len()) {
                    Some(k) if ui::REC_STEP_ROWS.contains(&k) => {
                        // A running recording keeps its settings.
                        if crate::csvlog::active() {
                            self.beep(now, 300);
                            self.toast(now, "Stop the recording first", false);
                            return;
                        }
                        if crate::fastlog::active() {
                            self.beep(now, 300);
                            self.toast(now, "Stop the recording first", false);
                            return;
                        }
                        let (_, mut di) = crate::csvlog::next();
                        let mut ii = self.rec_ival() as u8;
                        let d = ui::record_step(x) as i16;
                        let step = |v: u8, n: usize| (v as i16 + d).clamp(0, n as i16 - 1) as u8;
                        match k {
                            0 => ii = step(ii, ui::REC_INTERVALS.len()),
                            1 => crate::csvlog::set_change(step(crate::csvlog::change(), ui::REC_CHANGES.len())),
                            _ => di = step(di, ui::REC_DURATIONS.len()),
                        }
                        self.rec_fast = ii as usize == ui::REC_FAST;
                        let ii = if self.rec_fast { crate::csvlog::next().0 } else { ii };
                        crate::csvlog::set_next(ii, di);
                    }
                    Some(ui::REC_START_ROW) if self.rec_fast || crate::fastlog::active() => {
                        if self.record_fast_tap(now, clk) {
                            return;
                        }
                    }
                    Some(ui::REC_START_ROW) => {
                        let was = crate::csvlog::active();
                        self.rec_toggle(now);
                        // Start returns to the DMM page it came from.
                        if !was && crate::csvlog::active() {
                            let v = self.back_view;
                            self.show(v, clk);
                            return;
                        }
                    }
                    _ => return,
                }
                self.redraw = true;
            }
            View::Settings if self.set_page == 1 => {
                if ui::title_back_hit(y) {
                    self.set_page = 0;
                    self.redraw = true;
                    return;
                }
                match ui::settings_hit(1, y) {
                    Some(0) => {
                        self.dmm.set_rate(next_rate(self.dmm.rate()));
                    }
                    Some(1) => {
                        let s = Smooth::from_u8((self.meter.smoothing() as u8 + 1) % 3).unwrap_or(Smooth::Off);
                        self.meter.set_smoothing(s);
                    }
                    Some(2) => self.dmm_view.bar_on = !self.dmm_view.bar_on,
                    Some(3) => {
                        let f = !self.dmm.temp_f();
                        self.set_temp_f(f);
                    }
                    Some(4) => self.sp.swipes = !self.sp.swipes,
                    Some(5) => self.sp.press_fx = !self.sp.press_fx,
                    Some(6) => {
                        // Scope tap step: Corners -> Halves (stock) -> Off.
                        use crate::scope_ui::{TAP_CORNERS, TAP_HALVES, TAP_OFF};
                        self.sp.tap_mode = match self.sp.tap_mode {
                            TAP_CORNERS => TAP_HALVES,
                            TAP_HALVES => TAP_OFF,
                            _ => TAP_CORNERS,
                        };
                    }
                    Some(7) => {
                        self.help_page = 0;
                        self.show(View::Help, clk);
                        return;
                    }
                    Some(8) => self.set_page = 2,
                    Some(_) | None => return,
                }
                self.redraw = true;
            }
            View::Settings if self.set_page == 2 => {
                if ui::title_back_hit(y) {
                    self.set_page = 1;
                    self.redraw = true;
                    return;
                }
                match ui::settings_hit(2, y) {
                    Some(0) => {
                        // Jack alerts: ON (all) -> A-JACK only -> OFF -> ON.
                        use crate::sentinel::{M_AJACK, M_JACK};
                        let j = match self.dmm.alert_mask() & M_JACK {
                            M_JACK => M_AJACK,
                            M_AJACK => 0,
                            _ => M_JACK,
                        };
                        self.dmm.set_alert_mask(clk, j);
                    }
                    Some(1) => {
                        // Danger alarm (doc 4.10): Loud -> Volume -> Off.
                        self.alarm_mode = (self.alarm_mode + 1) % 3;
                    }
                    Some(2) => {
                        // USB disk (decision 2026-10-22 #6): Files live <->
                        // PC can edit, volatile; in force at the next bus
                        // reset (the host reads the WP bit when it mounts).
                        let ro = !usb_ro().1;
                        #[cfg(feature = "vdisk")]
                        crate::vwrite::set_read_only(ro);
                        if ro != usb_ro().0 {
                            // The row's new value first: a toast holds the
                            // page's repaint until it ends.
                            self.draw_static();
                            self.toast(clk.now_ms(), "Re-plug USB to apply", true);
                        }
                    }
                    Some(3) => {
                        // Burden voltage in the A functions (doc 3.15):
                        // from the next reading on.
                        self.dmm.burden_show = !self.dmm.burden_show;
                    }
                    Some(4) => {
                        self.show(View::Device, clk);
                        return;
                    }
                    Some(5) => {
                        self.tt_open(clk);
                        return;
                    }
                    Some(6) => {
                        // Accuracy label (doc dmm-accuracy.md): from the
                        // next reading on.
                        self.dmm_view.acc_on = !self.dmm_view.acc_on;
                    }
                    Some(7) => {
                        // Negative ohm: from the next reading on.
                        self.dmm.ohm_neg = !self.dmm.ohm_neg;
                    }
                    Some(8) => {
                        // Mode Remember: applies at the next power-on.
                        self.mode_remember = !self.mode_remember;
                    }
                    Some(_) | None => return,
                }
                self.redraw = true;
            }
            View::Settings => {
                if ui::title_back_hit(y) {
                    // Tapping the title bar ("< Back") returns to the menu.
                    self.show(View::Menu, clk);
                    return;
                }
                match ui::settings_hit(0, y) {
                    Some(0) => self.dim_idx = (self.dim_idx + 1) % 4,
                    Some(1) => self.off_idx = (self.off_idx + 1) % 4,
                    // -/+ steppers (ui-review #12): Backlight on the
                    // 10, 15 ... 95, 100 grid, volume 10 % steps, clamped.
                    Some(2) => {
                        self.backlight = ui::backlight_step(self.backlight, ui::settings_step(x));
                        backlight(self.backlight);
                    }
                    Some(3) => {
                        let d = ui::settings_step(x) as i16 * 10;
                        self.volume = (self.volume as i16 + d).clamp(0, 100) as u8;
                    }
                    Some(4) => self.bt_on = !self.bt_on,
                    Some(5) => self.start_page = (self.start_page + 1) % 4,
                    Some(6) => {
                        let b = !self.rails_both;
                        self.set_rails_both(b, clk);
                    }
                    Some(7) => {
                        // Scope drag: Stock (per-event) -> Axis lock -> Hold
                        // 300 ms (hold-to-grab, doc §6.2) -> Stock.
                        if gesture::grab_ms() != 0 {
                            gesture::set_grab_ms(0);
                            self.sp.drag_mode = scope_page::DRAG_STOCK;
                        } else if self.sp.drag_mode == scope_page::DRAG_STOCK {
                            self.sp.drag_mode = scope_page::DRAG_AXIS;
                        } else {
                            self.sp.drag_mode = scope_page::DRAG_STOCK;
                            gesture::set_grab_ms(gesture::GRAB_DEFAULT_MS);
                        }
                    }
                    Some(8) => {
                        // Last row: page 2 (DMM rate, smoothing, Device).
                        self.set_page = 1;
                    }
                    Some(_) | None => return,
                }
                self.redraw = true;
            }
        }
        let _ = dmm::button_of;
    }
}

impl App {
    /// After a DMM function / sub-mode change (function bar touch, locked
    /// PD6 click, SCPI CONF / MEAS / FUNC, BT command 06): HOLD, REL, statistics and the
    /// AutoHold window restart (meter.reset), the trend chart starts over
    /// for the new function, and the static DMM view (function bar, labels,
    /// chart axes) is redrawn. One path, so touch and SCPI give the same
    /// screen.
    /// `m` (as for select_func) is the current function (incl. the sensor).
    pub fn is_func(&self, m: u8) -> bool {
        let f = self.dmm.func();
        m == f || (m >= 32 && f == 15 && self.dmm.t_sensor() == m - 32)
    }

    /// The placeholder (`prov` with no value) follows a range / unit change
    /// made before the function's first reading: `CONF:CURR:DC 10` enters
    /// mA (`select`), then the 10 A row (device 6bc494a: "mA" for 0.8 s).
    fn prov_follow(&mut self) {
        let Some(p) = self.prov.filter(|p| p.main.v.is_nan() && !p.main.ol) else { return };
        let Some(n) = self.dmm.placeholder() else { return };
        if (n.mode, n.range, n.main.unit, n.main.dec) != (p.mode, p.range, p.main.unit, p.main.dec) {
            self.prov = Some(n);
            if matches!(self.view, View::Dmm | View::DmmScope) {
                self.redraw = true;
            }
        }
    }

    /// The range went from manual to AUTO (`was_auto` false; the AUTO key,
    /// the range page's AUTO / AUTO+ item, SCPI `<f>:RANG:AUTO ON` /
    /// `CONF:<f>`): the manual range's reading is no AUTO reading, so the
    /// placeholder (AUTO, dimmed dashes) until AUTO's first one, as after a
    /// function change. Before, the last manual reading stayed with MANU in
    /// the status bar (the label is the reading's) until AUTO's first
    /// reading: through a whole ohm autorange hunt on the device (c6a3ac3
    /// note 2: 0.1043 Mohm for 8 s).
    fn auto_entered(&mut self, was_auto: bool) {
        if !was_auto && self.dmm.is_auto() && !self.dmm.pw.on {
            self.prov = self.dmm.placeholder();
        }
    }

    fn func_changed(&mut self) {
        // A power meter dialog (wiring, Reset energy?, Zero / V cal) or the
        // fuse check's start / result dialog belongs to the page it was
        // opened on: a function change from SCPI / BT closes it (#104, #143;
        // it stayed over a page no longer redrawn). The fuse check's
        // measuring stage (1) has its own end path (`fuse_tick`).
        if matches!(
            self.overlay,
            Some(
                crate::dialog::Overlay::PowerEntry { .. }
                    | crate::dialog::Overlay::EnergyReset { .. }
                    | crate::dialog::Overlay::PowJob { .. }
                    | crate::dialog::Overlay::FuseCheck { stage: 0 | 2, .. }
            )
        ) {
            self.overlay_close();
        }
        self.meter.reset();
        // The new function's frame at once (switch-latency.md proposal 1):
        // its unit and dimmed dashes until the first reading.
        self.prov = self.dmm.placeholder();
        self.chart_n = 0;
        // No unit until the new function's first reading (the labels are
        // numbers only, never the previous function's unit).
        self.chart_unit = "";
        self.chart_dirty = true;
        self.redraw = true;
    }

    /// A slider owns the current touch (main.rs: capture it with
    /// gesture::Tracker::capture_direct, no hold-to-grab). No DMM page has
    /// a slider since the temperature offset became a stepper.
    pub fn slider_active(&self) -> bool {
        false
    }

    /// The temperature page's state (sensor, offset, unit, NTC user
    /// parameters, info line), then draw the page (None) or one of its keys
    /// (Some((code, pressed))). One function, not a closure parameter: four
    /// monomorphized copies of ~330 B each before (region C audit #1).
    #[inline(never)] // .cold (link-native.x): temperature page draw / taps
    fn with_temp_page(&self, key: Option<(u8, bool)>) {
        let k = self.dmm.t_sensor();
        let mut b = [0u8; 64];
        let n = self.temp_info_text(&mut b);
        let (r25, beta) = self.dmm.ntc_user();
        let p = ui::TempPage {
            sensor: k,
            offs: self.dmm.t_offs(k),
            unit_f: self.dmm.temp_f(),
            r25,
            beta,
            info: core::str::from_utf8(&b[..n]).unwrap_or(""),
        };
        match key {
            None => ui::temp_page(&p),
            Some((c, pressed)) => ui::temp_key(&p, c, pressed),
        }
    }

    /// The temperature page's info line into `b` (its length): the sensor
    /// current of a resistive sensor, else the cold junction.
    #[inline(never)] // .cold (link-native.x): temperature page only
    fn temp_info_text(&self, b: &mut [u8; 64]) -> usize {
        let mut n = 0;
        let mut put = |t: &[u8]| {
            let m = t.len().min(64 - n);
            b[n..n + m].copy_from_slice(&t[..m]);
            n += m;
        };
        let mut t = [0u8; 16];
        if let Some((_, _, i, _)) = self.dmm.t_excitation() {
            // Estimated sensor current (dmm.rs t_excitation; inferred).
            put(b"Sensor current ~");
            put(dmm::format(&mut t, i * 1e3, 3).as_bytes());
            put(b" mA");
        } else if let Some((cj, _)) = self.dmm.t_cj(u64::MAX) {
            let (v, u) = self.dmm.t_disp(cj);
            put(b"Cold junction ");
            put(dmm::format(&mut t, v, 1).as_bytes());
            put(if u == "\u{b0}F" { b" F" } else { b" C" });
        }
        n
    }

    /// Temperature page, once a second: the info line again when its text
    /// changed (the cold junction follows each thermocouple reading and an
    /// offset step; it stayed as drawn when the page opened). `drawn`: the
    /// whole page was just drawn, only note its text.
    #[inline(never)] // .cold (link-native.x): temperature page, once a second
    fn temp_info_tick(&mut self, drawn: bool) {
        let mut b = [0u8; 64];
        let n = self.temp_info_text(&mut b);
        // FNV-1a of the text.
        let h = b[..n].iter().fold(0x811c_9dc5u32, |h, &c| (h ^ c as u32).wrapping_mul(0x0100_0193));
        if h != self.temp_info_h {
            self.temp_info_h = h;
            if !drawn {
                ui::temp_info(core::str::from_utf8(&b[..n]).unwrap_or(""));
            }
        }
    }

    /// Temperature page: the current sensor's offset (C, clamped to
    /// -20..20), redrawing only the value field.
    fn set_t_offs(&mut self, c: f32) {
        let k = self.dmm.t_sensor();
        self.dmm.set_t_offs(k, c);
        if self.view == View::Range && self.dmm.hy_mode() == 15 {
            self.with_temp_page(Some((ui::TK_OFFS, false)));
        }
    }

    /// The temperature page is shown.
    fn temp_page_on(&self) -> bool {
        self.view == View::Range && self.dmm.hy_mode() == 15
    }

    /// Global temperature unit (Settings 2/2, SCPI UNIT:TEMP): a change
    /// restarts the meter like a function change (REL, MIN/MAX, chart and
    /// smoothing hold deg values of the old unit) and converts the
    /// temperature limits, which are kept in the display unit.
    pub fn set_temp_f(&mut self, f: bool) {
        if f == self.dmm.temp_f() {
            return;
        }
        self.dmm.set_temp_f(f);
        let conv = |v: f32| if f { v * 1.8 + 32.0 } else { (v - 32.0) / 1.8 };
        let l = &mut self.meter.lim[crate::meter::lim_slot(15)];
        l.lo = l.lo.map(conv);
        l.hi = l.hi.map(conv);
        if self.dmm.hy_mode() == 15 {
            self.func_changed();
            self.last_raw = None;
        }
    }

    /// The DMM function was changed from outside the app (Bluetooth phone
    /// app, command 06 / Dmm::apply_mode_byte): the common path, as
    /// `press_func` / `select_func` after their Dmm call.
    pub fn host_func_changed(&mut self) {
        self.func_changed();
        self.last_raw = None;
    }

    /// SCPI CONFigure / MEASure / FUNCtion: select stock mode `m` (dmm.rs
    /// Dmm::select: 16 / 17 AC+DC, 32 + k temperature sensor k) as a touch
    /// on the function bar would.
    pub fn select_func(&mut self, clk: &mut Clock, m: u8) {
        if self.is_func(m) {
            return;
        }
        self.dmm.select(clk, m);
        self.func_changed();
        self.last_raw = None;
    }
}

/// SCPI ranges / resolution (scpi.rs Rng, Cmd::Range / Conf).
impl App {
    /// The present DMM range in SI (scpi::range_si); None outside V / A /
    /// ohm / capacitance / frequency and in the power meter. Region B (not
    /// .cold): CONF? asks it, also while the Wave view streams.
    pub fn range_si(&self) -> Option<f32> {
        if self.dmm.pw.on {
            return None;
        }
        let m = self.dmm.hy_mode();
        let r = if m == 14 { self.dmm.counter_range() as i8 } else { self.dmm.range() };
        crate::scpi::range_si(m, r)
    }

    /// [SENSe:]<f>:RANGe / :RANGe:AUTO / :RESolution, and CONFigure:<f>'s
    /// range part (`f`: the function the header names). There is one range
    /// state, the current function's: a set naming another function is
    /// "Settings conflict"; a query of another one is no error
    /// (RANG:AUTO? 1, RANG? / RES? +9.91E37). A set acts as the MF
    /// range page (Dmm::set_range, REL off); AUTO as the AUTO key (the
    /// meter's own AUTO / AUTO+ choice is kept). A bare VOLTage / CURRent
    /// (`scpi::ANY_CPL`) is that quantity in any coupling, in the power
    /// meter its V / I channel (queries only). Capacitance / frequency:
    /// AUTO or a hold of the present range only. The resolution is the
    /// FR-05 rate: FAST = 60000 counts, MED / SLOW = the +1 digit where the
    /// range has it (Dmm::rate_info). True: a reply.
    #[inline(never)] // .cold (link-native.x): one SCPI command
    fn scpi_range(&mut self, clk: &mut Clock, f: u8, op: crate::scpi::Rng, out: &mut crate::scpi::Scpi) -> bool {
        use crate::scpi::{self as s, Rng};
        let query = op.is_query();
        let cur = self.dmm.func();
        // A bare VOLTage / CURRent: the quantity in any coupling.
        let any = f & s::ANY_CPL != 0;
        let f = f & !s::ANY_CPL;
        if matches!(f, 11 | 12 | 15) || f >= 32 {
            out.set_err("Undefined header");
            return false;
        }
        if any && cur == dmm::pw::FUNC && matches!(op, Rng::Query | Rng::AutoQuery) {
            // The power meter's V / I channel (POW:VOLT:RANG? / POW:CURR:RANG?).
            let p = &self.dmm.pw;
            let (auto, v) = if f == 1 { (p.v_auto, dmm::pw::V_LIM[p.vr as usize]) } else { (p.i_auto, dmm::pw::I_LIM[p.ik as usize]) };
            match op {
                Rng::AutoQuery => out.put_bytes(if auto { b"1" } else { b"0" }),
                _ => out.put_f32(v),
            }
            return true;
        }
        let same = if any { s::quantity(f) == s::quantity(cur) } else { s::func_id(f) == s::func_id(cur) };
        let range = self.range_si().filter(|_| same);
        let Some(v) = range else {
            // Another function: a query is no error (as a 34401A answers
            // any function's): RANG:AUTO? 1 (selecting it starts in
            // autorange, Dmm::select / set_sub), RANG? / RES? +9.91E37 (no
            // range until it is selected). A set: "Settings conflict".
            match op {
                Rng::AutoQuery => out.put_bytes(b"1"),
                _ if query => out.put_bytes(s::NAN),
                _ => out.set_err("Settings conflict"),
            }
            return query;
        };
        let cap = cur == 13;
        let fine = self.dmm.rate() != dmm::Rate::Fast && self.dmm.rate_info(self.dmm.rate()).1;
        match op {
            Rng::Query => out.put_f32(v),
            Rng::AutoQuery => out.put_bytes(if self.dmm.is_auto() { b"1" } else { b"0" }),
            // 600 Mohm: its 6000-count step on the range (0.1 Mohm); the
            // reading's decimals follow the value (dmm.rs Ohm600::decimals).
            Rng::ResQuery if dmm::is_ohm600(self.dmm.hy_mode(), self.dmm.range()) => out.put_f32(1e5),
            Rng::ResQuery => out.put_f32(s::pow10f(s::step_exp(v, cap) - fine as i32)),
            Rng::Auto if self.dmm.is_auto() => {}
            _ if self.scope.wave_on() => out.set_err("Settings conflict"),
            Rng::Auto => {
                self.dmm.auto_key(clk);
                self.auto_entered(false);
                self.meter.rel_off();
            }
            Rng::Hold => self.dmm.hold_range(true),
            Rng::Value(x) => {
                // MAX: the top item; else the smallest that holds x.
                let items = s::range_items(cur);
                // Ohm MAX: 60 Mohm; 600 Mohm (experimental) only by a value
                // above it.
                let top = items.len().checked_sub(if cur == 10 { 2 } else { 1 });
                let i = if x.is_infinite() { top } else { items.iter().position(|&it| it >= x * 0.9999) };
                match i {
                    _ if matches!(cur, 13 | 14) => out.set_err("Illegal parameter value"),
                    Some(i) => {
                        self.dmm.set_range(clk, Some(s::range_item_code(cur, i)));
                        self.meter.rel_off();
                    }
                    None => out.set_err("Data out of range"),
                }
            }
            // 600 Mohm: its digits follow the value and its conversions are
            // fixed (dmm.rs Ohm600): a resolution sets nothing.
            Rng::Res(_) if dmm::is_ohm600(self.dmm.hy_mode(), self.dmm.range()) => {}
            Rng::Res(x) => {
                // MAX (inf) or at least the 60000-count step: FAST; MIN, DEF
                // (NaN) or finer: the +1 digit rate.
                let coarse = x >= s::pow10f(s::step_exp(v, cap)) * 0.999;
                let r = if coarse {
                    dmm::Rate::Fast
                } else if self.dmm.rate() == dmm::Rate::Fast {
                    dmm::Rate::Medium
                } else {
                    self.dmm.rate()
                };
                if r != self.dmm.rate() {
                    self.dmm.set_rate(r);
                    self.chart_dirty = true;
                }
            }
        }
        self.redraw |= !query;
        query
    }
}

/// FR-01 / FR-02 / FR-06 host interface for SCPI CALC:... (the command
/// parsing lives in scpi.rs, the dispatch in scpi_dmm).
impl App {
    /// CALC:AHOLD ON|OFF / CALC:AHOLD?
    pub fn set_ahold(&mut self, on: bool) {
        self.meter.set_ahold(on);
        self.redraw |= self.view == View::Limits;
    }

    pub fn ahold(&self) -> bool {
        self.meter.ahold
    }

    /// CALC:AVER:MIN? / MAX? / AVER? / SDEV? / COUNt?: (min, max, avg, sd)
    /// in SI of the current function and the sample count; None before a
    /// sample.
    pub fn stats(&self) -> (Option<(f32, f32, f32, f32)>, u32) {
        let s = &self.meter.stats;
        (s.si().map(|(a, b, c)| (a, b, c, s.sd().unwrap_or(0.0))), s.n)
    }

    /// CALC:AVER:CLEar (keeps a running recording session).
    pub fn stats_clear(&mut self) {
        self.meter.stats.clear();
        self.chart_dirty = true;
    }

    /// CALC:AVER:STATe ON|OFF: recording session (no idle shutdown); ON
    /// restarts the statistics as the touch control does.
    pub fn set_stats_rec(&mut self, on: bool) {
        if on && !self.meter.stats.rec {
            self.meter.stats.clear();
        }
        self.meter.stats.rec = on;
        self.chart_dirty = true;
    }

    pub fn stats_rec(&self) -> bool {
        self.meter.stats.rec
    }

    /// CALC:LIM:UPP / LOW <v> (SI) for the current function; None clears
    /// it. Setting a limit turns the check on (as on the touch page).
    pub fn set_limit(&mut self, upper: bool, v: Option<f32>) {
        let l = &mut self.meter.lim[crate::meter::lim_slot(self.dmm.func_shown())];
        let v = v.filter(|x| x.is_finite());
        if upper {
            l.hi = v;
        } else {
            l.lo = v;
        }
        l.on |= v.is_some();
        self.meter.recheck();
        self.redraw |= self.view == View::Limits;
    }

    /// CALC:LIM:UPP? / LOW? (None: not set).
    pub fn limit(&self, upper: bool) -> Option<f32> {
        let l = &self.meter.lim[crate::meter::lim_slot(self.dmm.func_shown())];
        if upper { l.hi } else { l.lo }
    }

    /// CALC:LIM:STATe ON|OFF / ?
    pub fn set_limit_state(&mut self, on: bool) {
        self.meter.lim[crate::meter::lim_slot(self.dmm.func_shown())].on = on;
        self.meter.recheck();
        self.redraw |= self.view == View::Limits;
    }

    pub fn limit_state(&self) -> bool {
        self.meter.lim[crate::meter::lim_slot(self.dmm.func_shown())].on
    }

    /// CALC:LIM:FAIL?: 1 = the displayed value is outside the limits, 0
    /// otherwise (also when no limit applies).
    pub fn limit_fail(&self) -> bool {
        matches!(self.meter.verdict(), crate::meter::V_HI | crate::meter::V_LO)
    }
}

impl App {

    /// SCPI temperature / system / statistics-limits commands (scpi.rs
    /// Cmd::Temp* / Sys* / Calc*); None for the scope / generator commands
    /// (every DMM-side variant needs an arm here: the scope path would power
    /// the scope rail for it).
    /// FETC:BURD? / DISP:BURD (doc 3.15), RES:NEG; true = a reply was written.
    #[inline(never)] // .cold (link-native.x): SCPI only
    fn burden_scpi(&mut self, cmd: crate::scpi::Cmd, out: &mut crate::scpi::Scpi) -> bool {
        use crate::scpi::Cmd as C;
        match cmd {
            C::DispBurden(Some(b)) => {
                self.dmm.burden_show = b;
                self.redraw |= self.view == View::Settings;
                false
            }
            C::DispBurden(None) => {
                out.put_bytes(if self.dmm.burden_show { b"1" } else { b"0" });
                true
            }
            C::ResNeg(Some(b)) => {
                self.dmm.ohm_neg = b;
                self.redraw |= self.view == View::Settings;
                false
            }
            C::ResNeg(None) => {
                out.put_bytes(if self.dmm.ohm_neg { b"1" } else { b"0" });
                true
            }
            C::ModeRemember(Some(b)) => {
                self.mode_remember = b;
                self.redraw |= self.view == View::Settings;
                false
            }
            C::ModeRemember(None) => {
                out.put_bytes(if self.mode_remember { b"1" } else { b"0" });
                true
            }
            _ => {
                // The latest pre-REL reading of a current function.
                match self.last_raw.as_ref().filter(|r| crate::sentinel::shunt_of(r.mode).is_some() && r.power.is_none()) {
                    Some(r) => match self.dmm.burden_of(r) {
                        Some(v) => out.put_f32(v),
                        None => out.put_bytes(b"+9.90000000E+37"),
                    },
                    None => {
                        out.set_err("Data stale");
                        out.put_bytes(b"+9.90000000E+37");
                    }
                }
                true
            }
        }
    }

    /// DBG:GENTRIG:LAG / DBG:GENTRIG:STAT? / DBG:CH2:LAG (the GEN and CH2
    /// channels' tuning replies): region C (link-native.x), out of
    /// `scpi_dmm`'s frame.
    #[inline(never)]
    fn dbg_gen(&mut self, cmd: crate::scpi::Cmd, out: &mut crate::scpi::Scpi) -> bool {
        use crate::scpi::Cmd as C;
        match cmd {
            C::DbgGenLag(Some(ns)) => {
                // ns -> ticks of the 200 MHz timer clock (5 ns).
                crate::scope::set_gen_lag((ns / 5.0) as i32);
                self.scope.refresh();
                false
            }
            C::DbgGenStat => {
                for (i, v) in crate::gen::stats().iter().enumerate() {
                    if i > 0 {
                        out.put_bytes(b",");
                    }
                    out.put_u32(*v);
                }
                true
            }
            C::DbgGenLag(None) => {
                out.put_f32(crate::scope::gen_lag() as f32 * 5.0);
                true
            }
            C::DbgCh2Lag(Some(us)) => {
                crate::scope_ch2::set_lag_us(us as i32);
                self.scope.refresh();
                false
            }
            C::DbgCh2Lag(None) => {
                out.put_f32(crate::scope_ch2::lag_us() as f32);
                true
            }
            _ => false,
        }
    }

    fn scpi_dmm(&mut self, clk: &mut Clock, cmd: crate::scpi::Cmd, out: &mut crate::scpi::Scpi) -> Option<bool> {
        use crate::scpi::Cmd as C;
        let k = self.dmm.t_sensor();
        let now = clk.now_ms();
        let r = match cmd {
            C::TempSensor(Some(k)) => {
                if k != self.dmm.t_sensor() {
                    self.dmm.set_t_sensor(clk, k);
                    self.func_changed();
                    self.last_raw = None;
                }
                false
            }
            C::TempSensor(None) => {
                out.put_bytes(dmm::T_NAMES[k as usize].as_bytes());
                true
            }
            C::TempOffs(Some(v)) => {
                self.dmm.set_t_offs(k, v);
                false
            }
            C::TempOffs(None) => {
                out.put_f32(self.dmm.t_offs(k));
                true
            }
            C::TempNtc(Some((r25, b))) => {
                self.dmm.set_ntc_user(r25, b);
                false
            }
            C::TempNtc(None) => {
                let (r25, b) = self.dmm.ntc_user();
                out.put_f32(r25);
                out.put_bytes(b",");
                out.put_f32(b);
                true
            }
            C::TempExc => match self.dmm.t_excitation() {
                Some((r, ohm, i, p)) => {
                    for (j, v) in [r as f32, ohm, i, p].iter().enumerate() {
                        if j > 0 {
                            out.put_bytes(b",");
                        }
                        out.put_f32(*v);
                    }
                    true
                }
                None => {
                    out.set_err("Settings conflict");
                    false
                }
            },
            C::TempUnit(Some(f)) => {
                self.set_temp_f(f);
                false
            }
            C::TempUnit(None) => {
                out.put_bytes(if self.dmm.temp_f() { b"F" } else { b"C" });
                true
            }
            C::TempCj => {
                match self.dmm.t_cj(now) {
                    Some((t, age)) => {
                        let t = self.dmm.t_disp(t).0;
                        // Fresh only while the thermocouple mode cycles (one
                        // CJ phase every ~2.2 s).
                        if !(self.dmm.mode() == 15 && k == 0 && !self.dmm.suspended()) {
                            out.set_err("Data stale");
                        }
                        out.put_f32(t);
                        out.put_bytes(b",");
                        out.put_f32(age as f32 / 1000.0);
                    }
                    None => {
                        out.set_err("Data stale");
                        out.put_bytes(b"+9.90000000E+37");
                    }
                }
                true
            }
            C::SysTemp => {
                out.put_f32(self.dmm.t_disp(self.power.mcu_t).0);
                true
            }
            C::ConfQuery if self.dmm.pw.on => {
                out.note_asked(self.dmm.func());
                self.pw_conf(out);
                true
            }
            C::ConfQuery if self.scope.wave_on() => {
                // The Wave view pauses the DMM poll: `meter.live()` is the
                // reading from before it (device d8e249d: "VOLT:AC +6" on
                // 200 mV/div = AC mV 600 mV). The streamed row's format
                // instead (dmm.rs poll_ac's unit / decimals).
                let r = self.dmm.range().max(0) as u8;
                // DC V / DC mV (1 / 3) stream since E7: their DC rows' format
                // (dmm.rs poll_dcv; 33b44bd answered "VOLT +6E-04" = uA).
                let (unit, dec) = match self.dmm.hy_mode() {
                    0 | 1 => ("V", 4 - r.min(3)),
                    2 | 3 => ("mV", 3 - r.min(1)),
                    4 => ("A", 4 - r.min(1)),
                    5 => ("mA", 3 - r.min(1)),
                    _ => ("\u{b5}A", 2 - r.min(1)),
                };
                out.put_conf_fmt(self.dmm.func(), self.range_si(), Some((unit, dec, false)));
                true
            }
            C::ConfQuery => {
                out.put_conf(self.dmm.func(), self.range_si(), self.meter.live());
                true
            }
            C::Range(f, op) => self.scpi_range(clk, f, op, out),
            C::Conf(f, rng, res) => {
                self.select_func(clk, f);
                if !matches!(f, 11 | 12) {
                    self.scpi_range(clk, f, rng, out);
                }
                if let Some(v) = res {
                    self.scpi_range(clk, f, crate::scpi::Rng::Res(v), out);
                }
                self.prov_follow();
                false
            }
            C::Pow(c) => self.pw_scpi(clk, c, out),
            C::SysRail(Some(b)) => {
                self.set_rails_both(b, clk);
                self.redraw |= self.view == View::Settings;
                false
            }
            C::SysRail(None) => {
                out.put_bytes(if self.rails_both { b"BOTH" } else { b"AUTO" });
                true
            }
            C::SysRailState => {
                out.put_bytes(if self.scope.dmm_rail() { b"1," } else { b"0," });
                out.put_bytes(if self.scope.powered() { b"1" } else { b"0" });
                for n in self.dmm.wd_n {
                    out.put_bytes(b",");
                    out.put_u32(n as u32);
                }
                true
            }
            C::CalcAhold(Some(on)) => {
                self.set_ahold(on);
                false
            }
            C::CalcAhold(None) => {
                out.put_bytes(if self.ahold() { b"1" } else { b"0" });
                true
            }
            C::CalcAver(k) => {
                let (v, n) = self.stats();
                match (k, v) {
                    (3, _) => crate::put_u32(out, n),
                    (_, Some((min, max, avg, sd))) => out.put_f32([min, max, avg, sd, sd][k.min(4) as usize]),
                    (_, None) => {
                        out.set_err("Data stale");
                        out.put_bytes(b"+9.90000000E+37");
                    }
                }
                true
            }
            C::CalcAverClear => {
                self.stats_clear();
                false
            }
            C::CalcAverState(Some(on)) => {
                self.set_stats_rec(on);
                false
            }
            C::CalcAverState(None) => {
                out.put_bytes(if self.stats_rec() { b"1" } else { b"0" });
                true
            }
            C::CalcLim(k, Some(v)) => {
                self.set_limit(k == 1, Some(v));
                false
            }
            C::CalcLim(k, None) => {
                match self.limit(k == 1) {
                    Some(v) => out.put_f32(v),
                    None => out.put_bytes(b"+9.90000000E+37"),
                }
                true
            }
            C::CalcLimClear => {
                self.set_limit(false, None);
                self.set_limit(true, None);
                false
            }
            C::CalcLimState(Some(on)) => {
                self.set_limit_state(on);
                false
            }
            C::CalcLimState(None) => {
                out.put_bytes(if self.limit_state() { b"1" } else { b"0" });
                true
            }
            C::CalcLimFail => {
                out.put_bytes(if self.limit_fail() { b"1" } else { b"0" });
                true
            }
            // FR-05 conversion rate / display smoothing.
            C::SensRate(Some(i)) => {
                if let Some(r) = dmm::Rate::from_u8(i) {
                    self.dmm.set_rate(r);
                }
                self.redraw |= self.view == View::Settings;
                self.chart_dirty = true;
                false
            }
            C::SensRate(None) => {
                out.put_bytes(dmm::RATE_NAMES[self.dmm.rate() as usize].as_bytes());
                true
            }
            C::Coup(amp) => {
                let b = if amp { Button::Amp } else { Button::Volt };
                let t: &[u8] = if self.dmm.button() != b {
                    b"DC"
                } else {
                    match self.dmm.sub() {
                        0 => b"DC",
                        1 => b"AC",
                        2 => b"ACDC",
                        _ if matches!(self.dmm.shown_mode(), 0 | 2 | 4..=6) => b"AUTO,AC",
                        _ => b"AUTO,DC",
                    }
                };
                out.put_bytes(t);
                true
            }
            C::SensRatePer => {
                out.put_f32(self.dmm.reading_period_ms() as f32 / 1000.0);
                true
            }
            C::CalcSmoState(Some(on)) => {
                if on != (self.meter.smoothing() != Smooth::Off) {
                    self.meter.set_smoothing(if on { Smooth::Low } else { Smooth::Off });
                }
                self.redraw |= self.view == View::Settings;
                false
            }
            C::CalcSmoState(None) => {
                out.put_bytes(if self.meter.smoothing() != Smooth::Off { b"1" } else { b"0" });
                true
            }
            C::CalcSmoResp(Some(l)) => {
                self.meter.set_smoothing(Smooth::from_u8(l).unwrap_or(Smooth::Low));
                self.redraw |= self.view == View::Settings;
                false
            }
            C::CalcSmoResp(None) => {
                out.put_bytes(if self.meter.smoothing() == Smooth::High { b"SLOW" } else { b"FAST" });
                true
            }
            C::DbgGrab(Some(ms)) => {
                gesture::set_grab_ms(ms);
                self.redraw |= self.view == View::Settings;
                false
            }
            C::DbgPinch(Some(m)) => {
                scope_page::set_pinch_mode(m);
                false
            }
            C::DbgPinch(None) => {
                crate::put_u32(out, scope_page::pinch_mode() as u32);
                true
            }
            C::DbgGrab(None) => {
                crate::put_u32(out, gesture::grab_ms() as u32);
                true
            }
            C::DbgReadout => {
                // Emulator tests (ohmhunt, autowaittest, switchtest): what
                // the DMM readout draws now, whatever the view.
                match self.shown_reading() {
                    Some(r) => {
                        out.put_bytes(if !r.prov { b"F," } else if r.main.v.is_nan() { b"P," } else { b"G," });
                        out.put_bytes(r.main.unit.as_bytes());
                        out.put_bytes(b",");
                        if r.main.v.is_nan() && !r.main.ol {
                            out.put_bytes(b"NAN");
                        } else {
                            out.put_reading(&r);
                        }
                    }
                    None => out.put_bytes(b"N,,NAN"),
                }
                out.put_bytes(b",");
                crate::put_u32(out, self.meter.hold_state() as u32);
                // The status bar's range label as drawn (ui.rs: the shown
                // reading's flags).
                out.put_bytes(match self.shown_reading() {
                    Some(r) if !r.auto => b",MANU",
                    Some(r) if r.auto_plus => b",AUTO+",
                    Some(_) => b",AUTO",
                    None => b",",
                });
                true
            }
            C::DbgTouchTest(on) => self.tt_scpi(clk, on, out),
            C::DbgTouchCal(op) => {
                // A change redraws the touch test page (its Cal lines).
                if op != 0 && self.view == View::TouchTest {
                    self.redraw = true;
                }
                app_touchtest::tc_scpi(op, out)
            }
            C::DbgView => {
                // <view>,<settings page 1..3>,<wave view on>,<trend chart
                // ms/div> (emulator tests: where a DBG:KEY / DBG:TAP landed;
                // settingstest.py).
                out.put_bytes(ui::view_name(self.view).as_bytes());
                out.put_bytes(b",");
                crate::put_u32(out, self.set_page as u32 + 1);
                out.put_bytes(if self.scope.wave_on() { b",1," } else { b",0," });
                crate::put_u32(out, self.chart_rate as u32 * 160);
                true
            }
            C::DbgGest => {
                // n;target,axis,delta,ms;... oldest first.
                crate::put_u32(out, self.gest_n as u32);
                for &(t, a, d, ms) in &self.gest_log[..self.gest_n] {
                    out.put_bytes(b";");
                    crate::put_u32(out, t as u32);
                    out.put_bytes(if a == 1 { b",V," } else { b",H," });
                    if d < 0 {
                        out.put_bytes(b"-");
                    }
                    crate::put_u32(out, d.unsigned_abs() as u32);
                    out.put_bytes(b",");
                    crate::put_u32(out, ms as u32);
                }
                // Positions now: trigger position (px), trigger row, CH1 pos.
                out.put_bytes(b"|POS ");
                for (k, v) in [self.scope.s.hpos, self.scope.s.trig_row, self.scope.s.pos].into_iter().enumerate() {
                    if k > 0 {
                        out.put_bytes(b",");
                    }
                    if v < 0 {
                        out.put_bytes(b"-");
                    }
                    crate::put_u32(out, v.unsigned_abs());
                }
                // The last AUTO-set's decisions (scope_page.rs autoset_log).
                out.put_bytes(b"|AUTO");
                scope_page::autoset_log(out);
                true
            }
            C::SysFuse(true) => {
                self.dmm.fuse_start(clk);
                self.func_changed();
                false
            }
            C::SysFuse(false) => {
                use crate::sentinel::FuseRes as F;
                let mut b = [0u8; 16];
                let (t, v) = match self.dmm.fuse_state() {
                    F::Idle => ("IDLE", None),
                    F::Busy => ("BUSY", None),
                    F::Ok(r) => ("OK,", Some(r)),
                    F::Open => ("OPEN", None),
                    F::Odd(r) => ("FAIL,", Some(r)),
                    F::NoReading => ("FAIL,NONE", None),
                };
                out.put_bytes(t.as_bytes());
                if let Some(r) = v {
                    out.put_bytes(crate::dmm::format(&mut b, r, 2).as_bytes());
                }
                true
            }
            C::StatQuesCond => {
                out.put_u32(crate::sentinel::ques_bits(self.dmm.sent.alert) as u32);
                true
            }
            C::SysAlert => {
                out.put_bytes(self.dmm.sent.alert.name().as_bytes());
                true
            }
            C::SysAlertEnable(Some(m)) => {
                self.dmm.set_alert_mask(clk, m);
                false
            }
            C::SysAlertEnable(None) => {
                out.put_u32(self.dmm.alert_mask() as u32);
                true
            }
            C::DbgSentVar(Some(v)) => {
                self.dmm.set_sent_variant(clk, v);
                false
            }
            C::DbgSentVar(None) => {
                out.put_u32(crate::sentinel::variant() as u32);
                true
            }
            C::SysAlarm(Some(m)) => {
                self.alarm_mode = m.min(2);
                self.redraw |= self.view == View::Settings && self.set_page == 2;
                false
            }
            C::SysAlarm(None) => {
                out.put_bytes(crate::danger::SCPI_NAMES[self.alarm_mode.min(2) as usize].as_bytes());
                true
            }
            C::SysBeepVol(Some(v)) => {
                self.volume = v.min(100);
                self.redraw |= self.view == View::Settings && self.set_page == 0;
                false
            }
            C::SysBeepVol(None) => {
                out.put_u32(self.volume as u32);
                true
            }
            C::DbgAlarm => {
                let a = &self.alarm;
                for (i, v) in [a.sounding() as u32, a.silenced() as u32, a.events, self.alarm_mode as u32].into_iter().enumerate() {
                    if i > 0 {
                        out.put_bytes(b",");
                    }
                    out.put_u32(v);
                }
                true
            }
            #[cfg(feature = "dbg-src")]
            C::DbgSrc(Some(r)) => {
                match self.dmm.src_req(clk, r) {
                    Ok(()) => self.host_func_changed(),
                    Err(e) => out.set_err(e),
                }
                false
            }
            #[cfg(feature = "dbg-src")]
            C::DbgSrc(None) => {
                self.dmm.src_text(out);
                true
            }
            #[cfg(not(feature = "dbg-src"))]
            C::DbgSrc(_) => false,
            #[cfg(feature = "dbg-sent")]
            C::DbgSentReg => {
                let mut b = [0u8; 72];
                let n = self.dmm.reg_text(&mut b);
                out.put_bytes(&b[..n]);
                true
            }
            C::DbgSentPrec(Some(ms)) => {
                self.dmm.precond_ms = ms;
                false
            }
            C::DbgSentPrec(None) => {
                out.put_u32(self.dmm.precond_ms as u32);
                true
            }
            c @ (C::DbgGenLag(_) | C::DbgGenStat | C::DbgCh2Lag(_)) => self.dbg_gen(c, out),
            C::DbgSentLog => {
                let s = &self.dmm.sent;
                out.put_u32(s.log_n);
                for (a, k, t) in s.log() {
                    out.put_bytes(b";");
                    out.put_bytes(a.name().as_bytes());
                    out.put_bytes(b",");
                    out.put_bytes(k.name().as_bytes());
                    out.put_bytes(b",");
                    out.put_u32(*t);
                }
                true
            }
            #[cfg(feature = "dbg-sent")]
            C::DbgSent => {
                let mut b = [0u8; 96];
                let n = self.dmm.sent.diag_text(&mut b);
                out.put_bytes(&b[..n]);
                true
            }
            #[cfg_attr(not(feature = "acdc-meth"), allow(unused_variables))]
            C::DbgAm(op, k, v) => {
                match op {
                    // METH / STAT / ZERO / R0 (the parser makes no 0..6
                    // without the feature `acdc-meth`).
                    #[cfg(feature = "acdc-meth")]
                    0 => self.dmm.am_set_meth(clk, k),
                    #[cfg(feature = "acdc-meth")]
                    1 => out.put_bytes(&[b'A' + self.dmm.am.meth]),
                    #[cfg(feature = "acdc-meth")]
                    2 => {
                        for (i, x) in self.dmm.am_stat().iter().enumerate() {
                            if i > 0 {
                                out.put_bytes(b",");
                            }
                            if matches!(i, 0..=3 | 6..=8 | 16 | 18) {
                                out.put_u32(*x as u32);
                            } else {
                                out.put_f32(*x);
                            }
                        }
                    }
                    #[cfg(feature = "acdc-meth")]
                    3 => self.dmm.am_set_zero(k, v),
                    #[cfg(feature = "acdc-meth")]
                    4 => {
                        let (ok, fixed, c) = self.dmm.am_zero();
                        out.put_u32(ok as u32);
                        out.put_bytes(b",");
                        out.put_u32(fixed as u32);
                        for (i, x) in c.iter().enumerate() {
                            out.put_bytes(if i == 0 { b";" } else { b"," });
                            out.put_f32(*x);
                        }
                    }
                    #[cfg(feature = "acdc-meth")]
                    5 => self.dmm.am.r0 = k,
                    7 => self.dmm.set_range(clk, Some(k)),
                    // DBG:ACDC:TRACE (the parser makes no 8 / 9 without the
                    // feature `acdc-trace`).
                    #[cfg(feature = "acdc-trace")]
                    8 => {
                        let mut b = [0u8; 500];
                        let n = self.dmm.tr.text(k as usize, &mut b, 500);
                        out.put_bytes(&b[..n]);
                    }
                    #[cfg(feature = "acdc-trace")]
                    9 => self.dmm.tr.arm(k),
                    #[cfg(feature = "acdc-meth")]
                    6 => out.put_u32(self.dmm.am.r0 as u32),
                    _ => {}
                }
                matches!(op, 1 | 2 | 4 | 6 | 8)
            }
            #[cfg(feature = "dbg-acdc")]
            C::DbgAcdc => {
                let mut b = [0u8; 400];
                let n = self.dmm.acdc_diag_text(&mut b);
                out.put_bytes(&b[..n]);
                true
            }
            _ => return None,
        };
        Some(r)
    }

    /// SCPI scope / generator command. Writes the reply (if any) to `out` and
    /// returns true when there is one. Never inlined into main.rs `run`
    /// (one call site: LLVM may otherwise fold its ~10 KB into the hot loop's
    /// function, region B).
    #[inline(never)]
    pub fn scpi(&mut self, clk: &mut Clock, cmd: crate::scpi::Cmd, out: &mut crate::scpi::Scpi) -> bool {
        use crate::scpi::Cmd as C;
        if matches!(cmd, C::FetchBurden | C::DispBurden(_) | C::ResNeg(_) | C::ModeRemember(_)) {
            return self.burden_scpi(cmd, out);
        }
        if let Some(r) = self.scpi_dmm(clk, cmd, out) {
            return r;
        }
        let now = clk.now_ms();
        let mut g = self.gen.s;
        let mut sc = self.scope.s;
        let scope_cmd =
            !matches!(cmd, C::GenFunc(_) | C::GenFreq(_) | C::GenVolt(_) | C::GenOffs(_) | C::GenDuty(_) | C::GenOut(_) | C::DiagDac | C::DiagPwm);
        if self.scope.wave_on() && matches!(cmd, C::ChScale(Some(_)) | C::ChCoup(Some(_)) | C::DbgHwTrig(..)) {
            // MCX-only settings (front-end range, coupling, comparator):
            // leave the wave view. Timebase, trigger, RUN / STOP and the
            // queries (:MEAS?, :WAV:DATA?) act on the wave view.
            self.wave_close(clk, true);
            sc = self.scope.s;
        }
        if scope_cmd && !self.scope.powered() {
            self.host_scope = true;
            self.update_rails(clk);
        }
        let sc0 = sc;
        let reply = match cmd {
            C::GenFunc(Some(w)) => {
                g.wave = Wave::ALL[w as usize % 4];
                None
            }
            C::GenFunc(None) => {
                out.put_bytes(match g.wave {
                    Wave::Sine => b"SIN",
                    Wave::Square => b"SQU",
                    Wave::Triangle => b"TRI",
                    Wave::Ramp => b"RAMP",
                });
                Some(())
            }
            C::GenFreq(Some(f)) => {
                // Stored as set (clamped 1 Hz .. 10 MHz); the output snaps it.
                g.freq_hz = gen::clamp_freq(f.max(1.0).min(gen::F_MAX as f32) as u32);
                None
            }
            C::GenFreq(None) => {
                // The frequency actually output: the set value clamped to
                // the wave's limit and quantised (UF-14 / UF-15).
                out.put_f32(self.gen.actual_hz());
                Some(())
            }
            C::GenVolt(Some(v)) => {
                g = g.with_amp(v);
                None
            }
            // FR-28: NaN = AUTO (stock 0..Vpp).
            C::GenOffs(Some(o)) => {
                g = g.with_offset(Some(o));
                None
            }
            C::GenOffs(None) => {
                out.put_f32(g.offset_v.unwrap_or(g.amp_v / 2.0));
                Some(())
            }
            C::GenVolt(None) => {
                out.put_f32(g.amp_v);
                Some(())
            }
            C::GenDuty(Some(d)) => {
                g.duty = (d.clamp(0.0, 100.0) * 100.0) as u16;
                None
            }
            C::GenDuty(None) => {
                // Effective (quantised) duty, as the generator page shows.
                out.put_f32(self.gen.out.duty_pct());
                Some(())
            }
            C::GenOut(Some(on)) => {
                g.on = on;
                None
            }
            C::GenOut(None) => {
                out.put_bytes(if g.on { b"1" } else { b"0" });
                Some(())
            }
            C::TimScale(Some(t)) => {
                sc.tb = scope::SDIV
                    .iter()
                    .position(|&d| d >= t * 0.999)
                    .unwrap_or(scope::TB_MAX)
                    .clamp(scope::TB_MIN, scope::TB_MAX);
                None
            }
            C::TimScale(None) => {
                out.put_f32(scope::SDIV[sc.tb]);
                Some(())
            }
            C::ChScale(Some(v)) => {
                // V/div at the probe tip (as the screen label), like TRIG:LEV.
                sc.range = scope::VDIV.iter().position(|&d| d * sc.probe >= v * 0.999).unwrap_or(9);
                None
            }
            C::ChScale(None) => {
                out.put_f32(self.scope.vdiv());
                Some(())
            }
            C::ChCoup(Some(ac)) => {
                sc.ac = ac;
                None
            }
            C::ChCoup(None) => {
                out.put_bytes(if sc.ac { b"AC" } else { b"DC" });
                Some(())
            }
            C::TrigLevel(Some(v)) => {
                // Volts at the probe tip, as TRIG:LEV? answers (probe
                // applied), kept as given: the row was the one below the
                // value (truncated), up to a row off, and an off-screen
                // level became the edge row's volts (#80).
                sc.trig_row = self.scope.set_trig_volts(v);
                None
            }
            C::TrigLevel(None) => {
                out.put_f32(self.scope.trig_volts());
                Some(())
            }
            C::TrigSlope(Some(r)) => {
                sc.rising = r;
                None
            }
            C::TrigSlope(None) => {
                out.put_bytes(if sc.rising { b"POS" } else { b"NEG" });
                Some(())
            }
            C::TrigSweep(Some(m)) => {
                sc.mode = [scope::TrigMode::Auto, scope::TrigMode::Normal, scope::TrigMode::Single][m as usize % 3];
                None
            }
            C::TrigSweep(None) => {
                out.put_bytes(match sc.mode {
                    scope::TrigMode::Auto => b"AUTO",
                    scope::TrigMode::Normal => b"NORM",
                    scope::TrigMode::Single => b"SING",
                });
                Some(())
            }
            C::TrigSource(Some(t)) => {
                sc.tsrc = t.min(scope::TSRC_GEN);
                None
            }
            C::TrigSource(None) => {
                out.put_bytes(if sc.tsrc == scope::TSRC_GEN { b"GEN" } else { b"CHAN1" });
                Some(())
            }
            C::Run => {
                sc.run = true;
                self.scope.run_on();
                None
            }
            C::Stop => {
                sc.run = false;
                None
            }
            C::Single => {
                sc.mode = scope::TrigMode::Single;
                sc.run = true;
                None
            }
            C::Meas(k) => {
                out.put_f32(self.meas[k as usize % 4]);
                Some(())
            }
            C::WavData => {
                // SCReen + BYTE (the default); the other forms are blocks
                // built by main.rs (scope_wav::data).
                out.put_bytes(b"#3400");
                let mut w = [0u8; 400];
                for (b, c) in w.iter_mut().zip(crate::scope_wav::screen().iter()) {
                    *b = (*c >> 2) as u8;
                }
                out.put_bytes(&w);
                Some(())
            }
            C::WavPoints(Some(n)) => {
                self.wav_points = n;
                None
            }
            C::WavFormat(Some(w)) => {
                self.wav_word = w;
                None
            }
            C::WavPoints(None) | C::WavFormat(None) | C::TrigStatus | C::AcqSrate | C::ChOffs(None) => {
                crate::scope_wav::query(self, cmd, out);
                Some(())
            }
            C::ChOffs(Some(v)) => {
                // Volts added to the signal (Rigol / libscopehal): the
                // centre row shows -v (Scope::v_centre = pos * 4 V/div / 141).
                sc.pos = crate::scope_wav::offset_pos(v, self.scope.vdiv());
                None
            }
            C::Digitize => {
                sc.run = true;
                None
            }
            C::DiagDac => {
                // min,max (V, 3.0 V reference), mid crossings, window (s)
                let (lo, hi, cross, cyc) = self.power.sample(4, 512);
                out.put_f32(lo as f32 * 3.0 / 4096.0);
                out.put_bytes(b",");
                out.put_f32(hi as f32 * 3.0 / 4096.0);
                out.put_bytes(b",");
                out.put_f32(cross as f32);
                out.put_bytes(b",");
                out.put_f32(cyc as f32 / 200e6);
                Some(())
            }
            C::DiagPwm => {
                // PB14 rising edges over 2 ms (tight loop on GPIOB->IDR), and
                // the PB12 output-path select level.
                let idr = 0x4002_0410 as *const u32;
                let cyc = || unsafe { core::ptr::read_volatile(0xE000_1004 as *const u32) };
                let t0 = cyc();
                let mut last = false;
                let mut edges = 0u32;
                while cyc().wrapping_sub(t0) < 400_000 {
                    let v = unsafe { core::ptr::read_volatile(idr) } & (1 << 14) != 0;
                    if v && !last {
                        edges += 1;
                    }
                    last = v;
                }
                let pb12 = unsafe { core::ptr::read_volatile(idr) } & (1 << 12) != 0;
                out.put_f32(edges as f32 / 0.002);
                out.put_bytes(if pb12 { b",PB12=1" } else { b",PB12=0" });
                Some(())
            }
            // Comparator trigger (hwtrig.rs, ../docs/history/hwtrig-integration.md H8);
            // a scope command: the scope rail is on for it (calibration).
            C::DbgHwTrig(op, on) => {
                let hw = &mut self.scope.hw;
                let put_i = |out: &mut crate::scpi::Scpi, v: i64| {
                    if v < 0 {
                        out.put_bytes(b"-");
                    }
                    crate::put_u32(out, v.unsigned_abs().min(u32::MAX as u64) as u32);
                };
                match op {
                    0 => {
                        let d = hw.debug();
                        let (mr, mf) = (hw.map(true), hw.map(false));
                        for (i, v) in d.iter().chain([hw.enabled() as u32, hw.available() as u32].iter()).enumerate() {
                            if i > 0 {
                                out.put_bytes(b",");
                            }
                            crate::put_u32(out, *v);
                        }
                        for v in [mr.0, mr.1, mf.0, mf.1] {
                            out.put_bytes(b",");
                            out.put_f32(v);
                        }
                        out.put_bytes(b",");
                        crate::put_u32(out, hw.post_ticks());
                        Some(())
                    }
                    1 => {
                        hw.set_enabled(on);
                        None
                    }
                    2 => {
                        match self.scope.hw_calibrate(now, clk) {
                            Ok(r) => {
                                let f = r.flip;
                                for (i, v) in [f[0][0], f[0][1], f[1][0], f[1][1]].iter().enumerate() {
                                    if i > 0 {
                                        out.put_bytes(b",");
                                    }
                                    crate::put_u32(out, *v as u32);
                                }
                                for v in [r.map[0].0, r.map[0].1, r.map[1].0, r.map[1].1, r.hyst, r.slope, r.codes[0], r.codes[1]] {
                                    out.put_bytes(b",");
                                    out.put_f32(v);
                                }
                            }
                            Err(e) => out.put_bytes(match e {
                                crate::hwtrig::CalError::NoComparator => b"NoComparator",
                                crate::hwtrig::CalError::NoFlip => b"NoFlip",
                                crate::hwtrig::CalError::OffsetNoEffect => b"OffsetNoEffect",
                                crate::hwtrig::CalError::Slope => b"Slope",
                            }),
                        }
                        Some(())
                    }
                    3 => {
                        let st = hw.stats;
                        crate::put_u32(out, st.hits);
                        out.put_bytes(b",");
                        crate::put_u32(out, st.misses);
                        for v in [st.delta_last, st.delta_min, st.delta_max] {
                            out.put_bytes(b",");
                            put_i(out, v as i64);
                        }
                        out.put_bytes(b",");
                        out.put_f32(if st.hits > 0 { st.delta_sum as f32 / st.hits as f32 } else { 0.0 });
                        Some(())
                    }
                    _ => {
                        hw.reset_stats();
                        None
                    }
                }
            }
            C::WavPre => {
                crate::scope_wav::preamble(self, out);
                Some(())
            }
            _ => None, // DMM / system commands: scpi_dmm
        };
        if matches!(cmd, C::DiagDac | C::DiagPwm) {
        } else if !scope_cmd && reply.is_none() {
            // Settings only: a query (OUTP?, SOUR:FREQ? ...) re-applied the
            // generator (stop_outputs + restart: a glitch on a running output)
            // and ran Gen::apply, region C code: device 519fb8a, OUTP? polled
            // during a 38400 S/s disk session = late 25, max_lat 21.1 us at
            // lat_pc Gen::apply, 29 samples lost in 10 s.
            self.gen_set(g);
        } else if !matches!(cmd, C::Meas(_) | C::WavData | C::WavPre | C::DbgHwTrig(..) | C::WavPoints(_) | C::WavFormat(_))
            && reply.is_none()
        {
            self.scope_set(sc, now);
            if self.sp_on() && self.sp.popup != crate::scope_ui::Popup::None && self.scope.s != sc0 {
                // A popup is open (#82): the page again with it (the graph
                // around its box, then the popup), so the top bar, the panel
                // and the popup's own values show the host's change now,
                // not when the popup closes.
                self.redraw = true;
            }
            if matches!(cmd, C::Digitize) {
                // After apply (a run change clears it): the acquisition it
                // restarted is the one that ends the run.
                self.scope.one_shot = true;
            }
        }
        reply.is_some()
    }
}
