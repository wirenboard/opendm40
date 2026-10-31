//! Power meter in the app (dmm::pw, ui::pw): entry dialog, DMM page taps,
//! the Power Settings page, the continuous-current alarm, settings words and
//! the SCPI commands (scpi::Pow). A child of app.rs.

use super::*;
use crate::dmm::pw::{self, PowerVals};
use crate::scpi::Pow;

/// Persisted words (persist::W_POWER..+10): p0 = SPHACAL (i8) | method << 8 |
/// burden comp OFF << 9 (written for older builds, ignored on load) | "At
/// load" ON << 10 (doc 3.16: opt-in; a record without bit 10, i.e. every
/// older one, loads OFF: their bit 9 = 0 meant "ON" and "never touched"
/// alike, so an explicit ON cannot be told from the old default) |
/// (discards + 1) << 12 (0 = default) | I limit in
/// 0.1 A << 16 (0 = default) | time limit s << 24 (0 = default, 255 = none);
/// p1..p3 = R_path per unit (f32 bits, 0 = the shunt value); p4 / p5 = the
/// V gains of 6 / 60 and 600 / 1000 V, u16 each (0 = factory-derived, else
/// 0.5 + n / 65536); p6 = the Y zero (i16) | zeros valid << 16 | wiring
/// dialog seen << 17 | coupling (pw::COUP_*) << 18; p7..p9 = the X zeros
/// of the 6 current ranges (i16). The DC row's AD1 zeros: persist::W_POWER_ZD
/// (`pw_zd_words`).
pub const PW_WORDS: usize = 10;
pub const PW_ZD_WORDS: usize = 3;

fn gain_bits(g: f32) -> u32 {
    if g > 0.0 { ((g - 0.5) * 65536.0 + 0.5).clamp(1.0, 65535.0) as u32 } else { 0 }
}

fn gain_of(n: u32) -> f32 {
    let n = n & 0xFFFF;
    let g = 0.5 + n as f32 / 65536.0;
    if n != 0 && (0.9..=1.1).contains(&g) { g } else { 0.0 }
}

impl App {
    /// Open the wiring dialog (the W function button until it was confirmed
    /// once, a long press on A, Power Settings > Wiring). Auto-cancel after
    /// a minute.
    pub(super) fn pw_open_entry(&mut self, now: u64) {
        self.overlay = Some(crate::dialog::Overlay::PowerEntry { until: now + 60_000 });
        self.redraw = true;
        ui::pw::entry_dialog(self.dmm.pw.i_cont, self.dmm.pw.t_lim);
        self.beep(now, 50);
    }

    /// While the wiring dialog is open: its current path's dashes march at
    /// 10 fps (one repaint of the path wires per 100 ms).
    pub(super) fn pw_entry_tick(&mut self, now: u64, can_draw: bool) {
        use core::sync::atomic::{AtomicU32, Ordering};
        static PH: AtomicU32 = AtomicU32::new(0);
        if !can_draw || !matches!(self.overlay, Some(crate::dialog::Overlay::PowerEntry { .. })) {
            return;
        }
        let ph = (now / 100) as u32;
        if PH.swap(ph, Ordering::Relaxed) != ph {
            ui::pw::entry_anim(ph);
        }
    }

    /// The dialog's OK: enter the power meter on the DMM page.
    pub(super) fn pw_enter(&mut self, clk: &mut Clock) {
        self.overlay = None;
        self.dmm.pw.wired = true;
        self.dmm.select(clk, pw::FUNC);
        self.func_changed();
        self.last_raw = None;
        self.show(View::Dmm, clk);
    }

    /// DMM page / settings page taps of the power meter; true = consumed.
    pub(super) fn pw_tap(&mut self, clk: &mut Clock, x: u16, y: u16) -> bool {
        let now = clk.now_ms();
        match self.view {
            View::Dmm if ui::pw::side_box_hit(x, y) => {
                self.dmm.pw.run = !self.dmm.pw.run;
                self.chart_dirty = true;
                true
            }
            // The main value: P -> Wh -> VAh (display only; the next reading
            // redraws it).
            View::Dmm if ui::value_hit(x, y) => {
                let p = &mut self.dmm.pw;
                p.main = (p.main + 1) % 3;
                self.chart_n = 0;
                self.redraw = true;
                true
            }
            // The side column and the power boxes have no other tap action
            // (no MIN / MAX session, no history slot).
            View::Dmm if (x >= 402 && (175..279).contains(&y)) || ui::pw::row_hit(y) => true,
            View::Range => {
                let pv = self.dmm.pw_state();
                match ui::pw::settings_hit(x, y) {
                    Some(c @ ui::pw::PS_V..=4) => {
                        let v = if c == 4 { None } else { Some(c) };
                        self.dmm.pw_set_ranges(clk, Some(v), None);
                    }
                    Some(c @ ui::pw::PS_I..=14) => {
                        let i = c - ui::pw::PS_I;
                        let i = if i == 6 { None } else { Some(i) };
                        self.dmm.pw_set_ranges(clk, None, Some(i));
                    }
                    Some(ui::pw::PS_BURDEN) => {
                        self.dmm.pw.comp = !pv.comp;
                        self.pw_help = ui::pw::HELP_LOAD;
                    }
                    Some(c @ ui::pw::PS_COUP..=30) => {
                        self.dmm.pw_set_coupling(clk, ui::pw::PS_COUPS[(c - ui::pw::PS_COUP) as usize]);
                        self.pw_help = ui::pw::HELP_COUP;
                    }
                    // Zero... / V cal...: a dialog says what the job needs
                    // first; Start runs it (app.rs, Overlay::PowJob).
                    Some(c @ (ui::pw::PS_ZERO | ui::pw::PS_VCAL)) => {
                        let job = if c == ui::pw::PS_ZERO { pw::JOB_ZERO } else { pw::JOB_VCAL };
                        self.overlay = Some(crate::dialog::Overlay::PowJob { until: now + crate::dialog::ENERGY_MS, job });
                        self.redraw = true;
                        ui::pw::job_dialog(job);
                        return true;
                    }
                    Some(ui::pw::PS_RESET) => {
                        self.dmm.pw.energy_reset();
                        self.beep(now, 300);
                    }
                    Some(ui::pw::PS_WIRING) => {
                        // Shown again for reference; OK / Cancel just close it.
                        self.pw_open_entry(now);
                        return true;
                    }
                    Some(ui::pw::PS_LIMITS) => {
                        self.show(View::Limits, clk);
                        return true;
                    }
                    Some(ui::pw::PS_RECORD) => {
                        self.record_open(clk);
                        return true;
                    }
                    Some(ui::pw::PS_OK) => {
                        let v = self.back_view;
                        self.show(v, clk);
                        return true;
                    }
                    _ => return true,
                }
                self.redraw = true;
                true
            }
            _ => false,
        }
    }

    /// Long press on the energy readout / the side column in the power
    /// meter: the reset confirm (dialog.rs; Yes resets Wh, VAh, the time).
    pub(super) fn pw_long_side(&mut self, now: u64) {
        self.overlay = Some(crate::dialog::Overlay::EnergyReset { until: now + crate::dialog::ENERGY_MS });
        self.redraw = true;
        crate::dialog::energy_reset();
        self.beep(now, 50);
    }

    /// Per tick: the continuous-current limit expired -> toast + the alert
    /// beep pattern while it stays above (A-JACK's); auto-off is held while
    /// the energy integrates (`App::tick`).
    pub(super) fn pw_tick(&mut self, now: u64) {
        if !self.dmm.pw.on {
            self.pw_coup_seen = 255;
            self.pw_pending = 0;
            return;
        }
        self.pw_settings_status();
        // A coupling change (the W button, Power Settings, SCPI POW:COUP):
        // the last reading is of the old coupling. Drop it (as a function
        // change does: `func_changed`) and show the new coupling's page
        // with "--" until its first reading (device check 92fb8fa: the old
        // layout and values stayed up to ~4 s).
        let coup = self.dmm.pw.coup;
        if self.pw_coup_seen != coup {
            if self.pw_coup_seen != 255 {
                self.pw_coup_changed();
            }
            self.pw_coup_seen = coup;
        }
        // The coupling changed (SCPI, the W button): its glyph and the page
        // layout (AC+DC parts, doc 3.17; also the fallback method) redraw.
        if (self.dmm.pw.coup != self.pw_btn_coup || self.pw_parts_want() != ui::pw_parts()) && matches!(self.view, View::Dmm) {
            self.redraw = true;
        }
        if self.meter.shown().is_some() {
            self.pw_pending = 0;
        }
        // DMM page before the first full reading: V and "I ranging..."
        // (the I channel walks down from 10 A, ~0.9 s per range); after a
        // coupling change the pending page instead (no job running).
        if !self.redraw && self.meter.shown().is_none() {
            match self.view {
                View::Dmm => self.pw_no_reading(),
                View::DmmScope => self.pw_strip_partial(),
                _ => {}
            }
        }
        if self.dmm.pw_over_expired() {
            self.toast(now, "Current too high: reduce it", false);
            self.alert_beep_t0 = Some(now);
        }
        if self.alert_beep_t0.is_some() && self.dmm.pw_state().over != 2 {
            self.alert_beep_t0 = None;
        }
    }

    /// `pw_tick`, the DMM page without a reading: the pending page once,
    /// else the partial V / state text (region B: every pass while the
    /// page waits).
    #[inline(never)]
    fn pw_no_reading(&mut self) {
        if self.pw_pending != 0 && self.dmm.pw.job == 0 {
            if self.pw_pending == 1 {
                self.pw_pending = 2;
                self.pw_pending_draw();
            }
        } else if let Some((v, txt)) = self.dmm.pw_partial() {
            self.pw_rng_key = ui::pw::ranging(v, txt, self.pw_rng_key);
        }
    }

    /// The combined view's meter strip before the first power reading
    /// (#108; the I channel walks down from 10 A, ~0.9 s per range): POWER,
    /// the range badge and dimmed dashes as after any other function change,
    /// the V once measured (the DMM page's `pw_no_reading`).
    pub(super) fn pw_strip_wait(&self) -> crate::dmm::Reading {
        let mut r = self.dmm.pw_pending().0;
        r.prov = true;
        r.auto = self.dmm.pw.v_auto && self.dmm.pw.i_auto;
        r.aux1 = self.dmm.pw_partial().and_then(|p| p.0);
        r.aux2 = None;
        r
    }

    /// `pw_tick`, the combined view without a power reading: the strip again
    /// when the partial V changes (`pw_rng_key` as on the DMM page).
    #[inline(never)]
    fn pw_strip_partial(&mut self) {
        let k = match self.dmm.pw_partial() {
            Some((Some(v), _)) => v.v.to_bits() | 1,
            _ => 2,
        };
        if k != self.pw_rng_key {
            self.pw_rng_key = k;
            self.sp_strip();
        }
    }

    /// `pw_tick`: the coupling changed (any path).
    #[inline(never)] // .cold (link-native.x): once per coupling change
    fn pw_coup_changed(&mut self) {
        self.meter.reset();
        self.last_raw = None;
        self.chart_n = 0;
        self.chart_dirty = true;
        self.pw_pending = 1;
        self.redraw |= self.view == View::Dmm;
    }

    /// The pending page's values (`Dmm::pw_pending`) over the page that
    /// `draw_static` drew for the new coupling.
    #[inline(never)] // .cold (link-native.x): once per coupling change
    fn pw_pending_draw(&mut self) {
        let (r, pv) = self.dmm.pw_pending();
        self.dmm_view.alt = self.strip_alt;
        self.dmm_view.reading(&r, 0, crate::meter::V_NONE);
        ui::pw::side(&mut self.dmm_view.pwc, &pv);
    }

    /// The AC+DC parts layout applies (doc 3.17): the power meter in AC+DC
    /// with method B (the fallback has no parts).
    pub(super) fn pw_parts_want(&self) -> bool {
        let p = &self.dmm.pw;
        p.on && p.coup == pw::COUP_ACDC && !p.alt
    }

    /// The W button's coupling glyph for the next button draw.
    pub(super) fn pw_btn_glyph(&mut self) {
        self.pw_btn_coup = self.dmm.pw.coup;
        ui::set_power_coup(self.pw_btn_coup);
    }

    /// The shunt (0 A, 1 mA, 2 uA) whose R_path POW:BURDen sets / reads: in
    /// a DMM A function its own (the burden readout, doc 3.15), else the
    /// power meter's current range's (`ik`).
    #[inline(never)] // .cold (link-native.x): SCPI only
    fn burden_shunt(&self, ik: u8) -> u8 {
        match crate::sentinel::shunt_of(self.dmm.hy_mode()) {
            Some(u) if !self.dmm.pw.on => u,
            _ => pw::iu_ir(ik).0,
        }
    }

    /// Settings words (persist).
    pub(super) fn pw_words(&self) -> [u32; PW_WORDS] {
        let p = &self.dmm.pw;
        let ic = ((p.i_cont * 10.0 + 0.5) as u32).clamp(1, 100);
        let tl = if p.t_lim == 0 { 255 } else { p.t_lim.min(254) as u32 };
        let mut w = [0u32; PW_WORDS];
        w[0] = p.sph as u8 as u32 | (p.alt as u32) << 8 | (!p.comp as u32) << 9 | (p.comp as u32) << 10 | ((p.disc as u32 + 1) & 0xF) << 12 | ic << 16 | tl << 24;
        for i in 0..3 {
            w[1 + i] = if p.r_path[i] == pw::SHUNT_OHM[i] { 0 } else { p.r_path[i].to_bits() };
        }
        w[4] = gain_bits(p.vgain[0]) | gain_bits(p.vgain[1]) << 16;
        w[5] = gain_bits(p.vgain[2]) | gain_bits(p.vgain[3]) << 16;
        w[6] = p.zy as u16 as u32 | (p.zero_ok as u32) << 16 | (p.wired as u32) << 17 | (p.coup as u32 & 3) << 18;
        for i in 0..3 {
            w[7 + i] = p.zx[2 * i] as u16 as u32 | (p.zx[2 * i + 1] as u16 as u32) << 16;
        }
        w
    }

    /// The DC row's AD1 zeros (persist::W_POWER_ZD): 2 x i16 per word.
    pub(super) fn pw_zd_words(&self) -> [u32; PW_ZD_WORDS] {
        let z = &self.dmm.pw.zd;
        [0, 1, 2].map(|i| z[2 * i] as u16 as u32 | (z[2 * i + 1] as u16 as u32) << 16)
    }

    pub(super) fn pw_zd_restore(&mut self, w: &[u32]) {
        for i in 0..PW_ZD_WORDS {
            self.dmm.pw.zd[2 * i] = w[i] as u16 as i16;
            self.dmm.pw.zd[2 * i + 1] = (w[i] >> 16) as u16 as i16;
        }
    }

    pub(super) fn pw_restore(&mut self, w: &[u32]) {
        let p = &mut self.dmm.pw;
        p.sph = (w[0] & 0xFF) as u8 as i8;
        p.alt = w[0] >> 8 & 1 != 0;
        p.comp = w[0] >> 10 & 1 != 0;
        let d = (w[0] >> 12 & 0xF) as u8;
        p.disc = if d == 0 { pw::DISC_DEFAULT } else { (d - 1).min(8) };
        let ic = (w[0] >> 16 & 0xFF) as f32 / 10.0;
        p.i_cont = if ic == 0.0 { pw::I_CONT_DEFAULT } else { ic.min(10.0) };
        p.t_lim = match (w[0] >> 24) as u8 {
            0 => pw::T_LIM_DEFAULT,
            255 => 0,
            t => t as u16,
        };
        for i in 0..3 {
            let r = f32::from_bits(w[1 + i]);
            p.r_path[i] = if w[1 + i] != 0 && r.is_finite() && (0.0..=1000.0).contains(&r) { r } else { pw::SHUNT_OHM[i] };
        }
        p.vgain = [gain_of(w[4]), gain_of(w[4] >> 16), gain_of(w[5]), gain_of(w[5] >> 16)];
        p.zy = w[6] as u16 as i16;
        p.zero_ok = w[6] >> 16 & 1 != 0;
        p.wired = w[6] >> 17 & 1 != 0;
        // 2 (the former AUTO) loads as AC+DC (pw::coup_of).
        p.coup = pw::coup_of(w[6] >> 18);
        for i in 0..3 {
            p.zx[2 * i] = w[7 + i] as u16 as i16;
            p.zx[2 * i + 1] = (w[7 + i] >> 16) as u16 as i16;
        }
    }

    /// The power meter's SCPI commands. Queries answer from the latest
    /// values; outside the power meter the value queries are a "Settings
    /// conflict" (CONF:POW first), the settings work anywhere.
    pub(super) fn pw_scpi(&mut self, clk: &mut Clock, c: Pow, out: &mut crate::scpi::Scpi) -> bool {
        let on = self.dmm.pw.on;
        let pv = self.dmm.pw_state();
        let mut t = [0u8; 24];
        let put_i = |out: &mut crate::scpi::Scpi, t: &mut [u8; 24], v: i64| out.put_bytes(pw::i64_text(t, v));
        match c {
            Pow::Conf(v, i) => {
                if !on {
                    self.dmm_power(clk);
                    self.select_func(clk, pw::FUNC);
                }
                let m = |x: Option<u8>| x.map(|r| if r == 255 { None } else { Some(r) });
                self.dmm.pw_set_ranges(clk, Some(m(v).unwrap_or(None)), Some(m(i).unwrap_or(None)));
                self.redraw = true;
                false
            }
            Pow::All | Pow::Get(_) if !on => {
                out.set_err("Settings conflict");
                false
            }
            Pow::All => {
                // P,S,Q,PF,Vrms,Irms,F,E_Wh,seconds,Vdc,Idc,VAh,burden_V
                let vals = [pv.p, pv.s, pv.q, pv.pf, pv.v, pv.i, pv.f_hz, pv.e_wh, pv.secs as f32, pv.vdc, pv.idc, pv.vah, pv.burden_v];
                for (k, v) in vals.iter().enumerate() {
                    if k > 0 {
                        out.put_bytes(b",");
                    }
                    if v.is_finite() {
                        out.put_f32(*v);
                    } else {
                        out.put_bytes(b"+9.91000000E+37"); // NaN (SCPI)
                    }
                }
                true
            }
            Pow::Get(k) => {
                let v = [pv.s, pv.pf, pv.e_wh, pv.q, pv.v, pv.i][k.min(5) as usize];
                if v.is_finite() { out.put_f32(v) } else { out.put_bytes(b"+9.91000000E+37") }
                true
            }
            Pow::VRange(None) => {
                out.put_bytes(if pv.v_auto { b"AUTO," } else { b"MAN," });
                out.put_f32(pw::V_LIM[pv.vr as usize]);
                true
            }
            Pow::IRange(None) => {
                out.put_bytes(if pv.i_auto { b"AUTO," } else { b"MAN," });
                out.put_f32(pw::I_LIM[pv.ik as usize]);
                true
            }
            Pow::VRange(Some(r)) => {
                self.dmm.pw_set_ranges(clk, Some(r), None);
                self.redraw |= self.view == View::Range;
                false
            }
            Pow::IRange(Some(r)) => {
                self.dmm.pw_set_ranges(clk, None, Some(r));
                self.redraw |= self.view == View::Range;
                false
            }
            Pow::EnergyReset => {
                self.dmm.pw.energy_reset();
                false
            }
            Pow::EnergyState(Some(b)) => {
                self.dmm.pw.run = b;
                false
            }
            Pow::EnergyState(None) => {
                out.put_bytes(if pv.run { b"1" } else { b"0" });
                true
            }
            Pow::Burden(Some(r)) => {
                let iu = self.burden_shunt(pv.ik);
                self.dmm.pw.r_path[iu as usize] = if r.is_nan() { pw::SHUNT_OHM[iu as usize] } else { r };
                false
            }
            Pow::Burden(None) => {
                let iu = self.burden_shunt(pv.ik);
                out.put_f32(self.dmm.pw.r_path[iu as usize]);
                true
            }
            Pow::BurdenState(Some(b)) => {
                self.dmm.pw.comp = b;
                false
            }
            Pow::BurdenState(None) => {
                out.put_bytes(if pv.comp { b"1" } else { b"0" });
                true
            }
            Pow::Phase(Some(s)) => {
                self.dmm.pw_set_phase(clk, s);
                false
            }
            Pow::Phase(None) => {
                put_i(out, &mut t, self.dmm.pw.sph as i64);
                true
            }
            // RESistive is refused with the reason (pw::CAL_RES_REFUSAL,
            // doc 3.13); REACtive needs the power meter in method B.
            Pow::PhaseCal(Some(false)) => {
                out.set_err(pw::CAL_RES_REFUSAL);
                false
            }
            Pow::PhaseCal(Some(true)) => {
                if !self.dmm.pw_cal_start() {
                    out.set_err("Settings conflict; phase cal needs CONF:POW with AC or AC+DC coupling (P + PF method)");
                }
                false
            }
            // POW:PHAS:CAL?: the last outcome and SPHACAL ("OK,-3", "RUN,0",
            // "NOTREACTIVE,0").
            Pow::PhaseCal(None) => {
                out.put_bytes(pw::CAL_NAMES[self.dmm.pw.cal_res.min(5) as usize].as_bytes());
                out.put_bytes(b",");
                put_i(out, &mut t, self.dmm.pw.sph as i64);
                true
            }
            Pow::Cycle => {
                let (d, p, f) = self.dmm.pw_cycle();
                put_i(out, &mut t, d as i64);
                out.put_bytes(b",");
                put_i(out, &mut t, p as i64);
                out.put_bytes(b",");
                put_i(out, &mut t, f as i64);
                true
            }
            Pow::Method(Some(alt)) => {
                self.dmm.pw_set_method(clk, alt);
                self.redraw = true;
                false
            }
            Pow::Method(None) => {
                out.put_bytes(if pv.alt { b"ALT" } else { b"SIM" });
                true
            }
            Pow::Limit(Some((i, s))) => {
                self.dmm.pw.i_cont = i.max(0.1);
                self.dmm.pw.t_lim = s;
                false
            }
            Pow::Limit(None) => {
                out.put_f32(self.dmm.pw.i_cont);
                out.put_bytes(b",");
                put_i(out, &mut t, self.dmm.pw.t_lim as i64);
                true
            }
            Pow::Coupling(Some(c)) => {
                self.dmm.pw_set_coupling(clk, c);
                self.redraw |= self.view == View::Range;
                false
            }
            Pow::Coupling(None) => {
                out.put_bytes(match self.dmm.pw.coup {
                    pw::COUP_DC => b"DC",
                    pw::COUP_AC => b"AC",
                    _ => b"ACDC",
                });
                true
            }
            Pow::Zero(0) => {
                if !self.dmm.pw_zero_start(clk) {
                    out.set_err("Settings conflict");
                }
                self.redraw |= self.view == View::Range;
                false
            }
            Pow::Zero(1) => {
                let p = &mut self.dmm.pw;
                p.zero_ok = false;
                p.zx = [0; 6];
                p.zd = [0; 6];
                p.zy = 0;
                false
            }
            Pow::Zero(_) => {
                // <last job result>,<SET|NONE>,<Y zero>,<X zeros 600 uA..10 A>,
                // <DC-row AD1 zeros 600 uA..10 A>
                let p = &self.dmm.pw;
                out.put_bytes(job_text(if p.job_last == pw::JOB_ZERO { p.job_res } else { pw::JR_NONE }).as_bytes());
                out.put_bytes(if p.zero_ok { b",SET," } else { b",NONE," });
                put_i(out, &mut t, p.zy as i64);
                for z in p.zx.iter().chain(p.zd.iter()) {
                    out.put_bytes(b",");
                    put_i(out, &mut t, *z as i64);
                }
                true
            }
            Pow::VCal(0) => {
                if !self.dmm.pw_vcal_start(clk) {
                    out.set_err("Settings conflict");
                }
                self.redraw |= self.view == View::Range;
                false
            }
            Pow::VCal(1) => {
                let vr = self.dmm.pw.vr as usize;
                self.dmm.pw.vgain[vr] = 0.0;
                false
            }
            Pow::VCal(2) => {
                // <last job result>,<V range>,<gain in force>,<SELF|FACT>
                let p = &self.dmm.pw;
                let vr = if p.job_last == pw::JOB_VCAL { p.job_vr } else { p.vr };
                out.put_bytes(job_text(if p.job_last == pw::JOB_VCAL { p.job_res } else { pw::JR_NONE }).as_bytes());
                out.put_bytes(b",");
                out.put_f32(pw::V_LIM[vr as usize]);
                out.put_bytes(b",");
                out.put_f32(p.v_gain(vr));
                out.put_bytes(if p.vgain[vr as usize] > 0.0 { b",SELF" } else { b",FACT" });
                true
            }
            Pow::VCal(_) => {
                for vr in 0..4u8 {
                    if vr > 0 {
                        out.put_bytes(b",");
                    }
                    out.put_f32(self.dmm.pw.v_gain(vr));
                }
                true
            }
            Pow::DbgRow => {
                // 22 row bytes R20..R35 then R36, hex; the R34 in force.
                const H: &[u8; 16] = b"0123456789ABCDEF";
                let p = &self.dmm.pw;
                for (k, &b) in p.row.iter().chain(core::iter::once(&p.r36)).enumerate() {
                    if k > 0 {
                        out.put_bytes(b" ");
                    }
                    out.put_bytes(&[H[(b >> 4) as usize], H[(b & 15) as usize]]);
                }
                out.put_bytes(if on { b";ON" } else { b";OFF" });
                true
            }
            Pow::DbgPoke(i, v) => {
                if !self.dmm.pw_poke(clk, i, v) {
                    out.set_err("Illegal parameter value");
                }
                false
            }
            Pow::DbgStat(s) => {
                self.dmm.pw_freeze(clk, s);
                false
            }
            Pow::DbgRaw => {
                let n = self.dmm.pw.ring_text(&mut out.buf[..]);
                out.len = n;
                true
            }
            Pow::DbgDisc(Some(d)) => {
                self.dmm.pw.disc = d;
                false
            }
            Pow::DbgDisc(None) => {
                put_i(out, &mut t, self.dmm.pw.disc as i64);
                true
            }
        }
    }

    /// Power Settings page: the status line (job progress / result, the
    /// gain and zero in force), redrawn when its text changes.
    pub(super) fn pw_settings_status(&mut self) {
        if self.view != View::Range || !self.dmm.pw.on || self.redraw {
            return;
        }
        let k = ui::pw::settings_status(&self.dmm.pw, self.pw_status_key, self.pw_help);
        self.pw_status_key = k;
    }

    /// CONFigure? in the power meter: "POW <V range>,<I range>" (AUTO or the
    /// range limits in V / A).
    pub(super) fn pw_conf(&self, out: &mut crate::scpi::Scpi) {
        let pv: PowerVals = self.dmm.pw_state();
        out.put_bytes(b"\"POW ");
        if pv.v_auto {
            out.put_bytes(b"AUTO");
        } else {
            out.put_f32(pw::V_LIM[pv.vr as usize]);
        }
        out.put_bytes(b",");
        if pv.i_auto {
            out.put_bytes(b"AUTO");
        } else {
            out.put_f32(pw::I_LIM[pv.ik as usize]);
        }
        out.put_bytes(b"\"");
    }
}

/// SCPI word of a job result (pw::JR_*).
fn job_text(r: u8) -> &'static str {
    match r {
        pw::JR_RUN => "RUN",
        pw::JR_OK => "OK",
        pw::JR_SIGNAL => "SIGNAL",
        pw::JR_SMALL => "SMALL",
        pw::JR_BIG => "BIG",
        pw::JR_UNSTABLE => "UNSTABLE",
        pw::JR_RANGE => "RANGE",
        pw::JR_ABORT => "ABORT",
        pw::JR_METHOD => "METHOD",
        _ => "NONE",
    }
}
