//! The DMM as CH2 of the MCX scope page (docs/scope-gen-channel.md
//! section 2): the wave view's AD2 stream run alongside the MCX
//! acquisition and drawn in magenta, time-aligned (src/scope_ch2.rs).
//! main.rs starts / restarts / stops the stream (`ch2_want`, before the
//! wave view's and the inrush page's requests: their streams take over);
//! the DMM reading pauses while it runs, as in the wave view.

use super::*;
use crate::scope::WaveScale;
use crate::scope_ch2;

/// CH2's page state.
#[derive(Clone, Copy)]
pub struct Ch2 {
    /// Shown (MF > Display; the user's choice, kept).
    pub on: bool,
    /// The stream runs for CH2 (main.rs started it, `ch2_started`).
    pub live: bool,
    /// Its rate (S/s) as asked (`ch2_want`; a band change restarts it).
    pub fs: u32,
    /// V/div (A/div) of the CH2 trace: a `scope::wave_steps` value; 0 =
    /// the range's full scale at the next start.
    pub vdiv: f32,
    /// After a lost stream, no new start before this time (ms): a stream
    /// that cannot run must not rewrite the DMM row every pass.
    pub retry_at: u64,
    /// The store was frozen by a STOP: restart it at the next RUN (the
    /// samples in between were thrown away).
    pub frozen: bool,
}

/// CH2's highest stream rate (S/s): the 38 400 rung at 2 ms/div and
/// faster. At 76 800 (the wave view's rung up to 1 ms/div) the stream ISR
/// takes ~46 % of the CPU (2 ms/div: ~15 %) and the logger ring (2048
/// samples, 27 ms there) overflowed in the scope page's longer passes
/// (device: ~1500 samples/s dropped at 1 and 0.5 ms/div, 0 at 2 ms/div).
/// One rung for every timebase up to 2 ms/div also keeps one CH2 lag
/// (`DBG:CH2:LAG`) right for all of them.
pub const CH2_MAX_FS: u32 = 38_400;

impl Ch2 {
    pub const NEW: Ch2 = Ch2 { on: false, live: false, fs: 0, vdiv: 0.0, retry_at: 0, frozen: false };
}

/// The CH2 V/div label ("CH2 1.5V", "CH2 200mA") for the graph; the
/// bare V/div ("1.5V") for the Display tab.
fn label(vdiv: f32, amps: bool, prefix: bool) -> crate::scope_ui::Txt {
    let mut b = crate::scope_ui::Buf::new();
    if prefix {
        b.push("CH2 ");
    }
    crate::scope_ui::fmt_div(&mut b, vdiv, if amps { "A" } else { "V" });
    crate::scope_ui::Txt::new(b.as_str())
}

impl App {
    /// Why CH2 cannot run on the current DMM function (None: it can).
    fn ch2_why(&self) -> Option<&'static str> {
        let b = self.dmm.button();
        if !app_wave::WAVE_BUILD {
            Some("CH2: not in this build")
        } else if !matches!(b, Button::Volt | Button::Amp) || self.dmm.pw.on {
            Some("CH2: DMM in V or A")
        } else if self.dmm.sub() >= 2 {
            Some("CH2: not in AC+DC/AUTO")
        } else if b == Button::Amp && self.dmm.sub() == 0 {
            // DC A has no AD2 path (AD1 only); the wave view switches to the
            // AC A row, CH2 leaves the user's function alone.
            Some("CH2: needs AC A")
        } else if crate::csvlog::active() {
            Some("CH2: recording runs")
        } else if self.dmm.ad2_scale().is_none() {
            Some("CH2: no AD2 row here")
        } else {
            None
        }
    }

    /// main.rs: the rate CH2's stream should run at now, None = no stream
    /// for CH2 (off, another view, the wave view, a function without an
    /// AD2 row).
    pub fn ch2_want(&self) -> Option<u32> {
        if !self.ch2.on || self.view != View::Scope || self.scope.wave_on() || self.now < self.ch2.retry_at || self.ch2_why().is_some() {
            return None;
        }
        // Stopped: the store holds the stored record's CH2; a timebase
        // change (a stopped zoom) must not restart the stream at the new
        // rate, which empties the store (PR #64 device check: no CH2 in a
        // stopped zoom). RUN picks the new rate up.
        if !self.scope.s.run && self.ch2.live {
            return Some(self.ch2.fs);
        }
        Some(scope::wave_fs_for(self.scope.s.tb).min(CH2_MAX_FS))
    }

    /// main.rs: the stream for CH2 runs (or stopped / was lost: `live`
    /// false).
    pub fn ch2_live(&self) -> bool {
        self.ch2.live
    }

    pub fn ch2_fs(&self) -> u32 {
        self.ch2.fs
    }

    /// main.rs: the stream started at `fs` asked for, really `real_mhz`
    /// (mHz), its stamps counting from DWT `c0`.
    pub fn ch2_started(&mut self, fs: u32, real_mhz: u32, c0: u32) {
        let Some(a) = self.dmm.ad2_scale() else { return };
        if self.ch2.vdiv <= 0.0 || scope::wave_step_of(a.amps, self.ch2.vdiv).is_none() || self.scope_ch2_amps() != a.amps {
            self.ch2.vdiv = scope::wave_full_step(a.amps, a.item).map_or(a.full / 4.0, |i| scope::wave_steps(a.amps)[i].0);
        }
        let sc = WaveScale::of(a.per_code, a.offset, self.ch2.vdiv, a.amps, a.ac);
        let real = (real_mhz / 1000).max(1);
        scope_ch2::store().reset(c0, real, sc, crate::logger::wave::dropped());
        self.ch2.live = true;
        self.ch2.frozen = false;
        self.ch2.fs = fs;
    }

    fn scope_ch2_amps(&self) -> bool {
        scope_ch2::store().scale().amps
    }

    /// main.rs: CH2's stream stopped (CH2 off, the view left) or lost (a
    /// DMM change, a disk job): `ch2_want` starts it again when it can.
    pub fn ch2_stopped(&mut self) {
        self.ch2.live = false;
    }

    /// main.rs: CH2's stream ended without being asked to (the logger
    /// stopped it on a DMM change, the bus guard): again in 2 s.
    pub fn ch2_lost(&mut self, now: u64) {
        self.ch2.live = false;
        self.ch2.retry_at = now + 2000;
    }

    /// The stream into the CH2 store (every scope pass and between its
    /// slow parts, as the wave view's `wave_keep`). A stopped scope keeps
    /// the store as it was (the stored record's CH2) and throws the
    /// stream away.
    pub(super) fn ch2_drain(&mut self) {
        if !self.ch2.live {
            return;
        }
        let st = scope_ch2::store();
        if !self.scope.s.run {
            crate::logger::wave::drain(&mut |_| {});
            self.ch2.frozen = true;
            return;
        }
        let holes = crate::logger::wave::dropped();
        if holes != st.dropped() || self.ch2.frozen {
            st.restart(holes);
            self.ch2.frozen = false;
        }
        crate::logger::wave::drain_ts(&mut |ts, raw| st.push(ts, raw));
    }

    /// MF > Display "CH2 (DMM)" On / Off.
    pub(super) fn ch2_set(&mut self, clk: &mut Clock, on: bool) {
        let now = clk.now_ms();
        if on {
            if let Some(why) = self.ch2_why() {
                self.toast(now, why, false);
                return;
            }
            if self.scope.wave_on() {
                self.toast(now, "CH2: not in the Wave view", false);
                return;
            }
        }
        self.ch2.on = on;
        self.scope.ch2_on = on;
        if !on {
            crate::scope_ui::graph().c2 = [crate::scope_ui::EMPTY; 399];
        }
        // The comparator on / off: restart the acquisition.
        let st = self.scope.s;
        self.scope_set(st, now);
        self.update_rails(clk);
    }

    /// MF > Display CH2 V/div - / +: the next `scope::wave_steps` value
    /// (screen only: the DMM range stays).
    pub(super) fn ch2_step(&mut self, up: bool, now: u64) {
        let amps = self.scope_ch2_amps();
        let st = scope::wave_steps(amps);
        let i = scope::wave_step_of(amps, self.ch2.vdiv).unwrap_or(0);
        let j = if up { (i + 1 < st.len()).then_some(i + 1) } else { i.checked_sub(1) };
        match j {
            Some(j) => {
                self.ch2.vdiv = st[j].0;
                let mut sc = scope_ch2::store().scale();
                sc.vdiv = self.ch2.vdiv;
                scope_ch2::store().set_scale(sc);
                self.scope.refresh();
                self.sp_settings_changed();
            }
            None => self.toast(now, if up { "Highest V/div" } else { "Lowest V/div" }, false),
        }
    }

    /// The graph's CH2 label (empty: none); `prefix` false: the bare
    /// V/div of the Display tab ("--" before the first start).
    pub(super) fn ch2_label(&self, prefix: bool) -> crate::scope_ui::Txt {
        if self.ch2.on && self.ch2.vdiv > 0.0 && !self.scope.wave_on() {
            label(self.ch2.vdiv, self.scope_ch2_amps(), prefix)
        } else if prefix {
            crate::scope_ui::Txt::EMPTY
        } else {
            crate::scope_ui::Txt::new("--")
        }
    }

    /// CH2 rows of a frame (model rows per engine column, 0 V on the
    /// centre row at the CH2 V/div): from the stored record's time
    /// (`Scope::rec_time`, the frame's offset in it), or the live roll's
    /// (`roll_time`). Columns without CH2 samples (a zoomed-out frame,
    /// before the store) stay empty.
    pub(super) fn ch2_rows(&self, f: &scope::Frame, out: &mut [(i16, i16); 400]) {
        *out = [crate::scope_ui::EMPTY; 400];
        if !self.ch2.on || self.ch2.vdiv <= 0.0 || self.scope.wave_on() {
            return;
        }
        let st = scope_ch2::store();
        let k = 141.0 / (4.0 * self.ch2.vdiv);
        let row = |v: f32| (140.0 - v * k).clamp(-30000.0, 30000.0) as i16;
        // Columns left to right (rising times): one anchor search per frame
        // and f32 maths per column (`Store::span_run`).
        let mut run = st.run();
        let mut put = |i: usize, t0: u32, len: f32| {
            if let Some((lo, hi)) = st.span_run(&mut run, t0, len) {
                let (a, b) = (row(hi), row(lo));
                out[i] = (a.min(b), a.max(b));
            }
        };
        if f.cols.is_some() {
            let Some((end, per_col)) = self.scope.roll_time() else { return };
            for i in 0..scope::COLS {
                // Beyond ~10 s back: older than the store anyway (and the
                // DWT wrap).
                let back = (scope::COLS - i) as f32 * per_col;
                if back < 2e9 {
                    put(i, end.wrapping_sub(back as u32), per_col);
                }
            }
            return;
        }
        let n = f.data.len();
        let Some((t0, step)) = self.scope.rec_time() else { return };
        let rec = self.scope.record();
        let Some(off) = rec.offset_of(&f.data) else { return };
        if n < 2 {
            return;
        }
        // The frame's first sample; the columns' offsets from it in f32
        // (a frame spans < 2^31 cycles: ~1 us error at most).
        let t0 = t0.wrapping_add((off as f64 * step as f64) as u64 as u32);
        let kn = (n - 1) as f32 / (scope::COLS - 1) as f32;
        for i in 0..scope::COLS {
            let (a, b) = if n >= scope::COLS {
                ((i * n / scope::COLS) as f32, ((i + 1) * n / scope::COLS).max(i * n / scope::COLS + 1) as f32)
            } else {
                (i as f32 * kn, (i as f32 + 1.0) * kn)
            };
            put(i, t0.wrapping_add((a * step) as u32), ((b - a) * step).max(1.0));
        }
    }
}
