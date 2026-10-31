//! Pressed feedback (ui-review #11) on the menu tiles, the list pages'
//! rows / stepper keys and the generator boxes: the item under the finger
//! drawn pressed from touch-down until the touch ends or rolls off it (the
//! keypads' rule), one item redrawn per change. A child of app.rs; runs per
//! gesture, not per main-loop pass: linked into .cold (link-native.x).

use super::*;

impl App {
    /// Track the pressed item through gesture `g` (before the tap handlers).
    /// The guard is inlined into App::gesture (region B): every touch
    /// report of a drag on an instrument view (scope trigger / cursor drags
    /// while a stream runs) passes here and must not fetch from region C.
    /// Not while a stream ISR owns the HY3131 bus (a fast-log session:
    /// the screen is frozen, a DBG:TAP is the only input that reaches the
    /// pages; fastlogtest --ui-nav): this code is region C.
    #[inline(always)]
    pub(super) fn press_gesture(&mut self, g: &Gesture) {
        if self.pressed.is_some()
            || (matches!(self.view, View::Menu | View::Gen | View::Settings | View::Limits | View::Record | View::Range)
                && !crate::hy3131::streaming())
        {
            self.press_track(g);
        }
    }

    #[inline(never)]
    fn press_track(&mut self, g: &Gesture) {
        match g {
            Gesture::Down(p) => {
                self.press_end();
                if let Some(t) = self.press_hit(p.x, p.y) {
                    self.pressed = Some((self.view, t));
                    self.press_draw(t);
                }
            }
            Gesture::Drag(_, p, _) | Gesture::Move(_, p) if self.pressed.map(|q| q.1) == self.press_hit(p.x, p.y) => {}
            // A long press with no action of its own still taps on release.
            Gesture::Grab(_) | Gesture::LongPress(_) => {}
            // The tap's action first (`press_after_tap`): a tap that opens
            // another page does not draw the item released before it.
            Gesture::Tap(p) if self.pressed.map(|q| q.1) == self.press_hit(p.x, p.y) => {}
            _ => self.press_end(),
        }
    }

    /// The item a touch at (x, y) would act on (the tap handlers' hit
    /// tests), as pressed feedback (ui-review #11).
    fn press_hit(&self, x: u16, y: u16) -> Option<ui::RowPress> {
        let r = |row: usize, part: i8| Some(ui::RowPress { row: row as u8, part, on: true });
        match self.view {
            View::Menu => r(ui::main_menu_hit(x, y)?, 0),
            View::Gen if ui::gen_wave_hit(x, y) => r(4, 0),
            View::Gen => match ui::gen_box_hit(x, y)? {
                3 if self.gen.s.wave != Wave::Square => None,
                k => r(k, 0),
            },
            View::Settings | View::Limits | View::Record if ui::title_back_hit(y) => None,
            View::Settings => {
                let k = ui::settings_hit(self.set_page, y)?;
                r(k, if self.set_page == 0 && ui::SET_STEP_ROWS.contains(&k) { ui::settings_step(x) } else { 0 })
            }
            View::Limits => r(ui::list_hit(y, ui::LIMIT_ROWS.len())?, 0),
            // The temperature page has its own (`tp_pressed`).
            View::Range if self.dmm.hy_mode() == 15 => None,
            View::Range if self.dmm.pw.on => r(ui::pw::settings_hit(x, y)? as usize, 0),
            // MF range page: the tap handler's order (Wave, Inrush, the rest).
            View::Range => {
                let b = self.dmm.button();
                if app_wave::WAVE_BUILD && matches!(b, Button::Volt | Button::Amp) && ui::wave_hit(x, y) {
                    r(ui::RP_WAVE as usize, 0)
                } else if app_wave::WAVE_BUILD && b == Button::Amp && ui::inrush_hit(x, y) {
                    r(ui::RP_INRUSH as usize, 0)
                } else {
                    match ui::range_hit(b, self.range_sel(), x, y)? {
                        Ok(i) => r(i, 0),
                        Err(k) => r((ui::RP_ERR + k) as usize, 0),
                    }
                }
            }
            View::Record => match ui::list_hit(y, ui::RECORD_ROWS.len())? {
                k if ui::REC_STEP_ROWS.contains(&k) => r(k, ui::record_step(x)),
                ui::REC_START_ROW => r(ui::REC_START_ROW, 0),
                _ => None,
            },
            _ => None,
        }
    }

    /// Draw item `t` of the current view pressed / released: the page's
    /// own draw call limited to it (ui::set_row_only), no full repaint.
    fn press_draw(&mut self, t: ui::RowPress) {
        match self.view {
            View::Menu => ui::menu_item(t.row as usize, ui::view_name(self.combo), t.on),
            View::Gen => ui::gen_press(&self.gen, t.row as usize, t.on),
            View::Range if self.dmm.pw.on => ui::pw::settings_press(&self.dmm.pw_state(), t.row, t.on),
            View::Range => {
                let b = self.dmm.button();
                let sub = if b == Button::Ohm { self.dmm.ohm_var() } else { self.dmm.sub() };
                ui::range_press(b, t.row, self.range_sel(), sub, t.on);
            }
            _ => {
                ui::set_row_only(Some(t));
                self.draw_static();
                ui::set_row_only(None);
            }
        }
    }

    /// After a tap's action: the item drawn released, unless the page is
    /// left or repainted anyway.
    #[inline(never)]
    pub(super) fn press_after_tap(&mut self) {
        if self.redraw {
            self.pressed = None;
        } else {
            self.press_end();
        }
    }

    /// Release the pressed item (still on its view: drawn normal again).
    fn press_end(&mut self) {
        if let Some((v, mut t)) = self.pressed.take() {
            if v == self.view {
                t.on = false;
                self.press_draw(t);
            }
        }
    }
}
