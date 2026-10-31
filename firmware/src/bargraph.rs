//! Fast analog bargraph for the DMM page (UF-30, docs/user-feedback.md
//! section 4): a segmented bar under the readout, fed with every conversion
//! (`App::reading`, the un-smoothed `Reading::main`), so it moves at the
//! converter rate while the digits stay averaged / smoothed. The stock
//! firmware has no bargraph (no bar widget in gui_v2_6.h / the DMM layout).
//!
//! Look (owner 2026-10-21: a plain strip was "unclear what it is"): the
//! classic DMM LCD bargraph: SEGS discrete segments, a ruler made of the
//! segments themselves (every 5th segment, the scale's 10 % marks, full
//! height, the others MINOR_DY shorter), lit segments in the readout colour,
//! a centre zero for the bipolar (DC) scale, all segments red on overload.
//! The scale labels (0 / half / full scale of the range, signed for the
//! bipolar scale) and the overload marker are text, drawn by ui.rs
//! (DmmView) under / beside the bar.
//!
//! Rate (measured facts, dmm.rs FR-05 table): opendm40 has no separate fast
//! channel; a Reading arrives once per AD1 conversion: 10/s while moving and
//! in FAST (OSR 8192), 2.5/s once a MEDIUM / SLOW reading is steady (OSR
//! 32768); AC runs on the RMS converter's own rate. The >= 40/s asked for in
//! UF-30 needs AD1 at OSR 0x13 (the logger's ~38/s): open.
//!
//! Scale: the range's full scale (`full_scale`: the 60000-count format of
//! `Val::dec`, 1000 V / 10 A on the ranges the hardware limits; 1000 ohm /
//! 3 V in continuity / diode, the 6000-count capacitance ranges,
//! `mode_full_scale`).
//!
//! Drawing: `draw_static` once (every segment unlit), then `update` repaints
//! only the segments whose state changed (one fill_rect each).

use crate::lcd::{self, Color};

/// Unlit segment (a grey that reads as "off" on BG, like an LCD segment).
const OFF: Color = 0x31A6;
const FILL: Color = 0xFC40; // ui::VALUE
const OL: Color = 0xF800;
/// Number of segments (2 % of full scale each; bipolar: 4 % per side).
pub const SEGS: u16 = 50;
/// Minor segments start this many rows below the major ones.
const MINOR_DY: u16 = 3;

/// Full scale of the range a value with `dec` decimals in `unit` belongs
/// to: 60000 counts, except the 1000 V (DC V r3, 1 decimal) and 10 A
/// (DC / AC A r1, 3 decimals) ranges the front end limits.
pub fn full_scale(dec: u8, unit: &str) -> f32 {
    let mut s = 60000.0f32;
    for _ in 0..dec {
        s /= 10.0;
    }
    match (unit, dec) {
        ("V", 1) => 1000.0,
        ("A", 3) => 10.0,
        _ => s,
    }
}

/// Full scale of DMM `mode`: continuity (11) and diode (12) read up to
/// their OL ceilings (dmm.rs poll_cont: > 1000 ohm; poll_diode: > 3.12 V,
/// the 3 V scale), not the 60000-count scale of their format (600 ohm /
/// 6 V); capacitance (13): its range `range` (6 nF x 10^range, in nF on
/// r0..r2, else in uF: 600.0 nF, 6000 uF, 60000 uF; dmm.rs cap_val);
/// the experimental 600 Mohm (ohm r7, dmm::OHM600) its range, whatever
/// its decimals; else `full_scale`.
pub fn mode_full_scale(mode: u8, range: i8, dec: u8, unit: &str) -> f32 {
    match mode {
        11 => 1000.0,
        12 => 3.0,
        13 => {
            let mut s = 6.0f32;
            for _ in 0..(if unit == "nF" { range } else { range - 3 }).clamp(0, 7) {
                s *= 10.0;
            }
            s
        }
        10 if range == 7 => 600.0,
        _ => full_scale(dec, unit),
    }
}

/// Signed fraction of full scale `fs`; None = overload.
pub fn frac(v: f32, fs: f32, ol: bool) -> Option<f32> {
    if ol || !v.is_finite() || fs <= 0.0 {
        return None;
    }
    Some((v / fs).clamp(-1.0, 1.0))
}

/// A bar at (x, y), `w` x `h`: SEGS segments on a pitch of w / SEGS.
pub struct Bar {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub bipolar: bool,
    /// Lit segments [a, b) and overload; None = repaint all.
    shown: Option<(u16, u16, bool)>,
    /// Host tests: one probed item for the whole bar across updates.
    #[cfg(test)]
    pid: u32,
}

impl Bar {
    pub const fn new(x: u16, y: u16, w: u16, h: u16, bipolar: bool) -> Self {
        Bar {
            x,
            y,
            w,
            h,
            bipolar,
            shown: None,
            #[cfg(test)]
            pid: 0,
        }
    }

    fn pitch(&self) -> u16 {
        self.w / SEGS
    }

    /// Left edge and width of segment i (2 px gap after each); the scale's
    /// 10 % marks (every 5th segment boundary, and the last segment) are
    /// full height.
    pub fn seg_x(&self, i: u16) -> u16 {
        self.x + i * self.pitch()
    }

    /// Right end of the last segment (exclusive).
    pub fn end_x(&self) -> u16 {
        self.seg_x(SEGS - 1) + self.pitch() - 2
    }

    /// Centre column of the scale (between the two middle segments).
    pub fn mid_x(&self) -> u16 {
        self.seg_x(SEGS / 2) - 1
    }

    fn major(i: u16) -> bool {
        i % 5 == 0 || i == SEGS - 1 || i == SEGS / 2 - 1
    }

    fn seg(&self, i: u16, c: Color) {
        let dy = if Self::major(i) { 0 } else { MINOR_DY };
        lcd::fill_rect(self.seg_x(i), self.y + dy, self.pitch() - 2, self.h - dy, c);
    }

    /// Every segment unlit (and the gaps / minor tops in `bg`). Forces the
    /// next `update` to compare against the unlit state.
    pub fn draw_static(&mut self, bg: Color) {
        #[cfg(test)]
        {
            self.pid = crate::probe::open(crate::probe::SHAPE, bg);
            crate::probe::set(self.pid, bg, 0);
        }
        lcd::fill_rect(self.x, self.y, self.w, self.h, bg);
        for i in 0..SEGS {
            self.seg(i, OFF);
        }
        #[cfg(test)]
        crate::probe::set(0, bg, 0);
        let (a, b) = self.span(0.0);
        self.shown = Some((a, b, false));
    }

    /// Lit segments [a, b) for fraction `f`: unipolar from the left end,
    /// bipolar from the centre (SEGS / 2 per side), rounded to the nearest
    /// segment.
    fn span(&self, f: f32) -> (u16, u16) {
        if self.bipolar {
            let h = SEGS / 2;
            let k = ((f.abs() * h as f32 + 0.5) as u16).min(h);
            if f >= 0.0 { (h, h + k) } else { (h - k, h) }
        } else {
            (0, ((f.abs() * SEGS as f32 + 0.5) as u16).min(SEGS))
        }
    }

    /// Show fraction `f` (`frac`; None = overload: every segment red).
    pub fn update(&mut self, f: Option<f32>) {
        let (a, b, ol) = match f {
            Some(f) => {
                let (a, b) = self.span(f);
                (a, b, false)
            }
            None => (0, SEGS, true),
        };
        let lit = |i: u16, a: u16, b: u16| i >= a && i < b;
        #[cfg(test)]
        crate::probe::set(self.pid, crate::ui::BG, 0);
        match self.shown {
            Some((pa, pb, pol)) if pol == ol => {
                if !ol {
                    for i in a.min(pa)..b.max(pb) {
                        let now = lit(i, a, b);
                        if now != lit(i, pa, pb) {
                            self.seg(i, if now { FILL } else { OFF });
                        }
                    }
                }
            }
            _ => {
                for i in 0..SEGS {
                    self.seg(i, if ol { OL } else if lit(i, a, b) { FILL } else { OFF });
                }
            }
        }
        #[cfg(test)]
        crate::probe::set(0, 0, 0);
        self.shown = Some((a, b, ol));
    }
}
