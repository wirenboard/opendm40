//! Inrush current (TODO "Inrush current", as the Fluke 376 / 323;
//! docs/inrush-current.md): the page's open / arm / close, the end of a
//! capture (results, the save request), the saved file's header and its
//! record stream. Region C (`.cold`, link-native.x): none of this runs per
//! pass; what may run while the AD2 stream does (the ring drain, the live
//! state line, the page drawing, the SCPI queries a host polls) is in
//! app_inrush_live.rs / ui_inrush.rs (region B).
//!
//! Entry: Current Settings (MF in the A function) -> "Inrush" (builds with
//! the stream ISR, as the wave view), or SCPI INRush:STATe ON / :ARM. The
//! page selects the AC A row of the 10 A range (OP1 x1 on the 10 mohm
//! shunt, DC-coupled: AC and DC loads), lends the scope's record buffer
//! (`Scope::lend_buf`) and arms: main.rs starts the AD2 stream
//! (`Logger::start_wave` at inrush::RATE_HZ; the DMM reading pauses as in
//! the wave view), app_inrush_live.rs feeds every sample to the detector.
//! On the trigger + 1.8 s the stream stops, the results are computed
//! (inrush::Cap::results), the DMM resumes and the trace is saved as
//! LOGS/FASTnnn.DLG through the fast logger (fastlog.rs, a session whose
//! records come from the trace instead of the logger ring). Back / MENU /
//! MF closes the page: the A function's sub-mode and autorange come back,
//! the buffer goes back to the scope (after a save in progress).

use super::*;
use crate::inrush::{self as inr, Phase, Req, Save, Stop};
use crate::scpi::Inr;

/// The A function's MF list item of a current mode / range (manual).
fn a_item(mode: u8, range: i8) -> Option<u8> {
    let m = match mode {
        4..=6 => mode - 4,
        7..=9 => mode - 7,
        _ => return None,
    };
    Some((2 - m) * 2 + (range.max(0) % 2) as u8)
}

/// The zero code an arm starts from: the last measured (the one just
/// taken, kept for later arms: a load running at an arm keeps it), else the
/// calibration's (`inrush::ZERO_*`).
fn inr_zero(s: &mut inr::Session) -> f32 {
    if s.cap.zok {
        s.zm = s.cap.zf;
    }
    if s.zm.is_nan() { s.zf } else { s.zm }
}

impl App {
    /// Open the page (MF "Inrush", INR:STAT ON / INR:ARM): refused (toast,
    /// or the SCPI error `Err`) where it cannot run.
    #[inline(never)]
    pub(super) fn inrush_open(&mut self, clk: &mut Clock) -> Result<(), &'static str> {
        let now = clk.now_ms();
        let s = inr::ses();
        if s.on {
            return Ok(());
        }
        let why = if !app_wave::WAVE_BUILD {
            Some("Inrush: no stream in this build")
        } else if self.dmm.pw.on {
            Some("Inrush: not in the power meter")
        } else if crate::csvlog::active() || (crate::fastlog::active() && !s.saving()) {
            Some("Inrush: recording runs")
        } else if self.scope.held() {
            Some("Inrush: scope record in use")
        } else if s.saving() {
            Some("Inrush: still saving")
        } else {
            None
        };
        if let Some(t) = why {
            self.toast(now, t, false);
            return Err("Settings conflict");
        }
        if self.scope.wave_on() {
            self.wave_close(clk, false);
        }
        // What to restore on close (the stock A function's state).
        let amp = self.dmm.button() == Button::Amp;
        s.prev_func = self.dmm.func();
        s.prev_sub = if amp { self.dmm.sub() } else { 255 };
        s.prev_auto = (amp && self.dmm.is_auto()).then(|| self.dmm.a_auto());
        s.prev_item = if amp && !self.dmm.is_auto() { a_item(self.dmm.hy_mode(), self.dmm.range()) } else { None };
        self.meter.rel_off();
        self.scope.lend_buf();
        s.on = true;
        s.closing = false;
        s.res = None;
        s.save = Save::None;
        s.stop = Stop::None;
        s.traced = false;
        self.inrush_arm(clk);
        self.show(View::Inrush, clk);
        Ok(())
    }

    /// (Re)arm: the AC A row of the 10 A range, the detector reset, the
    /// stream (re)started by main.rs. While armed: only the threshold
    /// changes (in place, no restart).
    #[inline(never)]
    pub(super) fn inrush_arm(&mut self, clk: &mut Clock) {
        let s = inr::ses();
        if !s.on || s.saving() {
            return;
        }
        if self.dmm.button() != Button::Amp {
            self.select_func(clk, 4);
        }
        if self.dmm.sub() != 1 {
            self.dmm.set_sub(clk, 1);
            self.func_changed();
        }
        if !(self.dmm.hy_mode() == 4 && self.dmm.range() == 1 && !self.dmm.is_auto()) {
            self.dmm.set_range(clk, Some(5));
        }
        let Some(a) = self.dmm.ad2_scale() else {
            s.stop = Stop::Lost("Inrush: no AD2 row");
            self.redraw = true;
            return;
        };
        s.b = a.per_code * 8.0;
        s.zf = a.offset / s.b;
        let (z, k) = (inr_zero(s), inr::step_codes(s.thr, s.b));
        s.cap.arm(z, s.b, k);
        s.res = None;
        s.stop = Stop::None;
        s.traced = false;
        if s.save != Save::None {
            s.save = Save::None;
        }
        s.req = Some(Req::Start);
        self.redraw = true;
    }

    /// Cancel a capture in progress (the stream stops; nothing saved).
    #[inline(never)]
    pub(super) fn inrush_cancel(&mut self) {
        let s = inr::ses();
        if s.streaming {
            s.stop = Stop::Cancel;
            s.cap.phase = Phase::Off;
            s.req = Some(Req::Stop);
        }
    }

    /// The page closes (Back, MENU, MF, any view change: `show`): the
    /// stream stops first (main.rs), a save runs to its end, then the A
    /// function and the scope's buffer come back.
    #[inline(never)]
    pub(super) fn inrush_close(&mut self, clk: &mut Clock) {
        let s = inr::ses();
        if !s.on {
            return;
        }
        s.on = false;
        if s.streaming {
            s.closing = true;
            s.cap.phase = Phase::Off;
            s.req = Some(Req::Stop);
            return;
        }
        s.req = None;
        self.inrush_restore(clk);
    }

    /// The whole page (draw_static).
    #[inline(never)]
    pub(super) fn inrush_page(&mut self) {
        let s = inr::ses();
        let mut tb = [0u8; 24];
        let mut rb = [0u8; 16];
        let mut pb = [0u8; 16];
        let mut ib = [0u8; 16];
        let mut nb = [0u8; 48];
        let mut fb = [0u8; 7];
        let r = s.res;
        let rms = r.map_or("", |r| ui::inr::amps(&mut rb, r.rms100));
        let peak = match r {
            Some(r) if r.ol => "OL",
            Some(r) => ui::inr::amps(&mut pb, r.peak),
            None => "",
        };
        let idle = r.map_or("", |r| ui::inr::amps(&mut ib, r.idle));
        let file = match s.save {
            Save::Saved(n) if r.is_some() => fast_name(&mut fb, n),
            _ => "",
        };
        let (info, info_fg) = self.inrush_info(&mut nb);
        let (btn, btn_bg, enabled) = self.inrush_btn();
        ui::inr::page(&ui::inr::Vals {
            thr: thr_text(&mut tb, s.thr),
            rms,
            peak,
            info,
            info_fg,
            idle,
            file,
            btn,
            btn_bg,
            enabled,
        });
        let trace = if r.is_some() { self.scope.lent_buf().map(|b| &b[..]) } else { None };
        ui::inr::trace(trace, s.cap.zf, s.b);
        s.shown = self.inrush_key();
    }

    /// The state line after a capture / when stopped (no stream).
    #[inline(never)]
    pub(super) fn inrush_info_idle<'a>(&self, b: &'a mut [u8; 48]) -> (&'a str, crate::lcd::Color) {
        let s = inr::ses();
        let mut f = [0u8; 7];
        match s.cap.phase {
            _ if s.res.is_some() => {
                let lost = s.res.map_or(0, |r| r.lost);
                let mut lb = [0u8; 16];
                let ls: &str = if lost > 0 { crate::dmm::format(&mut lb, lost as f32, 0) } else { "" };
                let gaps: [&str; 2] = if lost > 0 { [", lost ", ls] } else { ["", ""] };
                match s.save {
                    Save::Pending | Save::Running => (super::app_inrush_live::cat(b, &["Captured: saving ...", gaps[0], gaps[1]]), ui::inr::C_OK),
                    Save::Saved(n) => (super::app_inrush_live::cat(b, &["Saved LOGS/", fast_name(&mut f, n), ".DLG", gaps[0], gaps[1]]), ui::inr::C_OK),
                    Save::Failed(c) => (
                        super::app_inrush_live::cat(b, &[
                            "Captured, not saved: ",
                            match c {
                                6 => "disk full",
                                7 => "no disk",
                                _ => "disk error",
                            },
                        ]),
                        ui::inr::C_WARN,
                    ),
                    Save::None => ("Captured", ui::inr::C_OK),
                }
            }
            _ => match s.stop {
                Stop::Lost(w) => (w, ui::inr::C_WARN),
                Stop::Cancel => ("Cancelled: press Arm", ui::inr::C_PLAIN),
                Stop::None => ("Press Arm, then switch the load on", ui::inr::C_PLAIN),
            },
        }
    }

    /// The A function as before the page; the buffer back to the scope
    /// unless the save still reads it (`inrush_saved` returns it then).
    fn inrush_restore(&mut self, clk: &mut Clock) {
        let s = inr::ses();
        s.closing = s.saving();
        s.cap.phase = Phase::Off;
        // Only what the page set (AC A, 10 A manual): a function a host or
        // the user chose meanwhile stays.
        let ours = self.dmm.hy_mode() == 4 && self.dmm.range() == 1 && !self.dmm.is_auto() && !self.dmm.pw.on && self.dmm.sub() == 1;
        if ours && s.prev_sub == 255 {
            self.select_func(clk, s.prev_func);
        } else if ours {
            if self.dmm.sub() != s.prev_sub {
                self.dmm.set_sub(clk, s.prev_sub);
                self.func_changed();
            }
            match (s.prev_auto, s.prev_item) {
                (Some(v), _) => self.dmm.set_range(clk, if v == 1 { Some(255) } else { None }),
                (None, Some(i)) => self.dmm.set_range(clk, Some(i)),
                _ => {}
            }
        }
        self.meter.reset();
        if !s.closing {
            self.scope.return_buf();
        }
    }

    /// main.rs: the stream runs (rate in mHz).
    #[inline(never)]
    pub fn inrush_live(&mut self, rate_mhz: u32) {
        let s = inr::ses();
        s.streaming = true;
        s.rate_mhz = rate_mhz;
        s.dropped = crate::logger::wave::dropped();
        s.shown = u32::MAX;
    }

    /// main.rs: the stream stopped at the page's request (`Req::Stop`):
    /// the capture's results and the save request, or the close.
    #[inline(never)]
    pub fn inrush_stopped(&mut self, clk: &mut Clock) {
        let now = clk.now_ms();
        let s = inr::ses();
        s.streaming = false;
        if s.closing {
            self.inrush_restore(clk);
            return;
        }
        if s.cap.phase == Phase::Done {
            if let Some(buf) = self.scope.lent_buf() {
                s.cap.linearize(buf);
                s.res = Some(s.cap.results(buf, s.b));
                s.traced = false;
                s.save = Save::Pending;
                self.fast_req = Some(FastReq::Inrush);
                self.beep(now, 150);
            }
        }
        s.cap.phase = if s.cap.phase == Phase::Done { Phase::Done } else { Phase::Off };
        self.redraw = true;
    }

    /// main.rs: the stream died behind the page's back (a host CONF, the
    /// bus guard, a host data logger) or could not start.
    #[inline(never)]
    pub fn inrush_lost(&mut self, clk: &mut Clock, why: &'static str) {
        let s = inr::ses();
        if !s.on && !s.closing {
            return;
        }
        let was = s.streaming || matches!(s.cap.phase, Phase::Arming | Phase::Armed | Phase::Post);
        s.streaming = false;
        s.req = None;
        if s.closing {
            self.inrush_restore(clk);
            return;
        }
        if was {
            s.cap.phase = Phase::Off;
            s.stop = Stop::Lost(why);
            self.redraw = true;
        }
    }

    /// A tap on the page (ui::inr::page_hit).
    #[inline(never)]
    pub(super) fn inrush_tap(&mut self, clk: &mut Clock, x: u16, y: u16) {
        let s = inr::ses();
        match ui::inr::page_hit(x, y) {
            Some(k @ (0 | 1)) => {
                s.thr = inr::thr_step(s.thr, if k == 0 { -1 } else { 1 });
                if matches!(s.cap.phase, Phase::Arming | Phase::Armed) {
                    // In place: the history and the stream go on.
                    s.cap.set_step(inr::step_codes(s.thr, s.b));
                }
                let mut b = [0u8; 24];
                ui::inr::thr(thr_text(&mut b, s.thr));
            }
            Some(2) => {
                let v = self.back_view;
                self.show(v, clk);
            }
            Some(3) => self.inrush_button(clk),
            _ => {}
        }
    }

    /// The main button / RUN key: Arm, or Cancel while capturing.
    #[inline(never)]
    pub(super) fn inrush_button(&mut self, clk: &mut Clock) {
        let s = inr::ses();
        if s.streaming {
            self.inrush_cancel();
        } else if !s.saving() {
            self.inrush_arm(clk);
        }
    }

    /// main.rs `fast_req`: the trace's file size (header + 3 B / sample,
    /// the fast logger's plan).
    pub fn inrush_save_bytes() -> u32 {
        (inr::LEN as u32) * crate::fastlog::BPS_AD2 + 8192
    }

    /// main.rs: the save could not start / the session began.
    #[inline(never)]
    pub fn inrush_save_begun(&mut self, ok: bool, now: u64) {
        let s = inr::ses();
        if ok {
            s.save = Save::Running;
        } else {
            self.inrush_saved(now, crate::fastlog::State::Error, None);
        }
    }

    /// main.rs (`fastlog::Ev::Start` of the inrush session): the file
    /// header (kind 3, as the fast logger's) with the capture's results.
    #[inline(never)]
    pub fn inrush_head(&mut self, out: &mut [u8; 512]) -> (usize, crate::dlog::FPar) {
        let s = inr::ses();
        let off = s.cap.zf * s.b;
        let par = crate::dlog::FPar {
            a: -off,
            b: s.b,
            rate_mhz: s.rate_mhz.max(1),
            flags: crate::dlog::FP_SCALED,
            src: 2,
            mode: 4,
            range: 1,
        };
        let mut w = crate::dlog::Txt { b: &mut out[..], n: 0 };
        let mut t = [0u8; 16];
        w.s("# Alientek ");
        w.s(crate::ident::model_name());
        w.s(", opendm40 ");
        w.s(crate::ident::VERSION);
        w.s(", inrush capture, AD2, AC A row 10 A (10 mohm, OP1 x1, DC-coupled), nominal ");
        w.u(s.rate_mhz as u64 / 1000);
        w.c(b'.');
        let f = s.rate_mhz % 1000;
        w.c(b'0' + (f / 100) as u8);
        w.c(b'0' + (f / 10 % 10) as u8);
        w.c(b'0' + (f % 10) as u8);
        w.s(" S/s\r\n# trigger: 1 ms mean |I| above the idle peak + ");
        w.s(crate::dmm::format(&mut t, s.thr, 3));
        w.s(" A; t0 at t_s ");
        w.s(crate::dmm::format(&mut t, inr::PRE as f32 / inr::RATE_HZ as f32, 6));
        if let Some(r) = s.res {
            w.s("; Irms(100 ms) ");
            w.s(crate::dmm::format(&mut t, r.rms100, 4));
            w.s(" A, peak ");
            if r.ol {
                w.s("OL");
            } else {
                w.s(crate::dmm::format(&mut t, r.peak, 3));
                w.s(" A");
            }
            w.s(", idle ");
            w.s(crate::dmm::format(&mut t, r.idle, 4));
            w.s(" A rms, lost ");
            w.u(r.lost as u64);
        }
        w.s("\r\n# t_s from the first sample (nominal rate); raw = AD2 code / 8; value = the DMM's AD2 calibration; lost_before = conversions missing before the row\r\n");
        w.s("t_s,raw,value_A,lost_before\r\n");
        (w.n, par)
    }

    /// The saved file's records (fastlog's drain closure): 12-byte logger
    /// records from the trace; 0 = all out.
    #[inline(never)]
    pub fn inrush_drain(&mut self, out: &mut [(u32, i32)]) -> usize {
        let s = inr::ses();
        match self.scope.lent_buf() {
            Some(buf) => s.cap.records(buf, out),
            None => 0,
        }
    }

    /// main.rs: the save ended (`fastlog::ended`).
    #[inline(never)]
    pub fn inrush_saved(&mut self, now: u64, st: crate::fastlog::State, file: Option<u16>) {
        use crate::fastlog::State;
        let s = inr::ses();
        s.save = match (st, file) {
            (State::Done | State::Full, Some(n)) => Save::Saved(n),
            _ => Save::Failed(st.code() as u8),
        };
        if s.closing {
            // The page closed meanwhile: the buffer back, a toast.
            s.closing = false;
            self.scope.return_buf();
            let mut t = *b"Inrush: saved FAST000.DLG";
            let txt = match s.save {
                Save::Saved(n) => {
                    t[18] = b'0' + (n / 100 % 10) as u8;
                    t[19] = b'0' + (n / 10 % 10) as u8;
                    t[20] = b'0' + (n % 10) as u8;
                    core::str::from_utf8(&t).unwrap_or("Inrush: saved")
                }
                _ => "Inrush: not saved",
            };
            self.toast(now, txt, matches!(s.save, Save::Saved(_)));
        }
        self.redraw = true;
    }

    /// INR:FETCh? / :THReshold? / :FILE? / :PREamble? and the setters
    /// (`inrush_cmd`); true: a reply.
    #[inline(never)]
    pub(super) fn inrush_scpi(&mut self, clk: &mut Clock, c: Inr, out: &mut crate::scpi::Scpi) -> bool {
        let s = inr::ses();
        out.len = 0;
        match c {
            Inr::State(None) => {
                out.put_bytes(if s.on { b"1" } else { b"0" });
                true
            }
            Inr::Cond => {
                out.put_bytes(match s.cap.phase {
                    Phase::Arming => b"ARMING",
                    Phase::Armed => b"ARMED",
                    Phase::Post => b"TRIGGERED",
                    _ if s.res.is_some() && s.saving() => b"SAVING",
                    _ if s.res.is_some() => b"DONE",
                    _ if s.on => b"IDLE",
                    _ => b"OFF",
                });
                true
            }
            Inr::Fetch => {
                match s.res {
                    Some(r) if !s.streaming => {
                        out.put_f32(r.rms100);
                        out.put_bytes(b",");
                        if r.ol {
                            out.put_bytes(b"+9.90000000E+37");
                        } else {
                            out.put_f32(r.peak);
                        }
                        out.put_bytes(b",");
                        out.put_f32(r.idle);
                        out.put_bytes(b",");
                        out.put_u32(r.lost);
                    }
                    _ => {
                        out.put_bytes(b"+9.90000000E+37,+9.90000000E+37,+9.90000000E+37,0");
                        out.set_err("Data stale");
                    }
                }
                true
            }
            Inr::Thr(None) => {
                out.put_f32(s.thr);
                true
            }
            Inr::File => {
                match (s.save, s.res.is_some()) {
                    (Save::Saved(n), true) => {
                        let mut f = [0u8; 7];
                        out.put_bytes(b"\"LOGS/");
                        out.put_bytes(fast_name(&mut f, n).as_bytes());
                        out.put_bytes(b".DLG\"");
                    }
                    _ => out.put_bytes(b"\"\""),
                }
                true
            }
            Inr::Pre => {
                // rate_hz, samples, trigger index, a, b (A = a + b * code).
                out.put_u32((s.rate_mhz + 500) / 1000);
                out.put_bytes(b",");
                out.put_u32(if s.res.is_some() { inr::LEN as u32 } else { 0 });
                out.put_bytes(b",");
                out.put_u32(inr::PRE as u32);
                out.put_bytes(b",");
                out.put_f32(-s.cap.zf * s.b);
                out.put_bytes(b",");
                out.put_f32(s.b);
                true
            }
            Inr::Data(..) => false,
            c => {
                if let Err(e) = self.inrush_cmd(clk, c) {
                    out.set_err(e);
                }
                false
            }
        }
    }

    /// INR:DATA? <start>[,<count>]: a #4nnnn block of the trace's 16-bit
    /// codes (little-endian i16, A = a + b * code, INR:PRE?), at most
    /// 3000 per block; an empty block without a capture.
    #[inline(never)]
    pub fn inrush_data(&self, start: u32, count: u32, buf: &mut [u8]) -> usize {
        let s = inr::ses();
        let trace = if s.res.is_some() && !s.streaming { self.scope.lent_buf() } else { None };
        let n = match trace {
            Some(_) => (count.min(3000) as usize).min(inr::LEN.saturating_sub(start as usize)).min((buf.len() - 6) / 2),
            None => 0,
        };
        let bytes = n * 2;
        buf[0] = b'#';
        buf[1] = b'4';
        for (i, d) in [1000usize, 100, 10, 1].iter().enumerate() {
            buf[2 + i] = b'0' + ((bytes / d) % 10) as u8;
        }
        if let Some(t) = trace {
            for i in 0..n {
                let c = t[start as usize + i];
                buf[6 + 2 * i..8 + 2 * i].copy_from_slice(&c.to_le_bytes());
            }
        }
        6 + bytes
    }

    /// INR:STATe ON / OFF, INR:ARM, INR:THReshold (the rest:
    /// app_inrush_live.rs `inrush_query`). Err: the SCPI error.
    #[inline(never)]
    pub fn inrush_cmd(&mut self, clk: &mut Clock, c: crate::scpi::Inr) -> Result<(), &'static str> {
        use crate::scpi::Inr as I;
        let s = inr::ses();
        match c {
            I::State(Some(true)) => {
                if !s.on {
                    self.inrush_open(clk)?;
                }
                Ok(())
            }
            I::State(Some(false)) => {
                if s.on {
                    let v = if self.view == View::Inrush { self.back_view } else { self.view };
                    if self.view == View::Inrush {
                        self.show(v, clk);
                    } else {
                        self.inrush_close(clk);
                    }
                }
                Ok(())
            }
            I::Arm => {
                if !s.on {
                    return self.inrush_open(clk);
                }
                if s.saving() {
                    return Err("Mass storage busy");
                }
                if s.streaming {
                    // A capture runs: start over (the stream goes on).
                    let (z, k) = (inr_zero(s), inr::step_codes(s.thr, s.b));
                    s.cap.arm(z, s.b, k);
                    s.res = None;
                    s.save = Save::None;
                    s.stop = Stop::None;
                    s.traced = false;
                    self.redraw = true;
                    return Ok(());
                }
                self.inrush_arm(clk);
                Ok(())
            }
            I::Thr(Some(a)) => {
                if !(inr::THR_MIN..=inr::THR_MAX).contains(&a) {
                    return Err("Data out of range");
                }
                s.thr = a;
                if matches!(s.cap.phase, Phase::Arming | Phase::Armed) {
                    s.cap.set_step(inr::step_codes(s.thr, s.b));
                }
                self.redraw |= self.view == View::Inrush;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// "+1.0 A" (the trigger stepper's value).
pub(super) fn thr_text<'a>(b: &'a mut [u8; 24], a: f32) -> &'a str {
    let mut t = [0u8; 16];
    let v = crate::dmm::format(&mut t, a, if a < 1.0 { 2 } else { 1 });
    let mut n = 0;
    for p in ["+", v, " A"] {
        let k = p.len().min(24 - n);
        b[n..n + k].copy_from_slice(&p.as_bytes()[..k]);
        n += k;
    }
    core::str::from_utf8(&b[..n]).unwrap_or("")
}

/// "FAST003" of a fast-logger file number.
pub(super) fn fast_name(b: &mut [u8; 7], n: u16) -> &str {
    *b = *b"FAST000";
    b[4] = b'0' + (n / 100 % 10) as u8;
    b[5] = b'0' + (n / 10 % 10) as u8;
    b[6] = b'0' + (n % 10) as u8;
    core::str::from_utf8(&b[..]).unwrap_or("")
}
