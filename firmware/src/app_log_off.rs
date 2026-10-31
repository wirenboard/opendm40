//! The frame log left out (feature `dec-log` off): DECode:MODE LOG and the
//! Decode tab's Mode Log are refused; the band decode is unchanged. The
//! full log is src/app_log.rs.

use super::*;

impl App {
    pub(super) fn log_open(&mut self, _clk: &mut Clock) -> Result<(), &'static str> {
        Err("Frame log not in this build")
    }
    pub(super) fn log_close(&mut self) {}
    pub(super) fn log_reset_decode(&mut self) {}
    pub(super) fn log_poll(&mut self, _now: u64) {}
    pub(super) fn log_page(&mut self) {}
    pub(super) fn log_tick(&mut self, _now: u64) {}
    pub(super) fn log_level(&self) -> Option<f32> {
        None
    }
    pub(super) fn log_rate(&self) -> u32 {
        0
    }
    pub(super) fn log_gesture(&mut self, _clk: &mut Clock, _g: Gesture) -> Option<bool> {
        None
    }
    pub(super) fn log_setup_changed(&mut self) {}
    pub(super) fn log_clear(&mut self) {}
    pub(super) fn log_count(&self) -> [u32; 6] {
        [0; 6]
    }
    pub(super) fn log_lines(&mut self, _max: u32, _b: &mut [u8]) -> usize {
        0
    }
}
