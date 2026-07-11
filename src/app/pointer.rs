use super::{input::InputController, App};
use crate::controls::PointerButton;
use crate::game::GameInput;

/// Wheel notches of travel per hotbar slot. One classic detent is `1.0`
/// (Windows' `WHEEL_DELTA` / 120, as winit normalizes it), so a notched wheel
/// still advances exactly one slot per click. Hi-res / free-spin mice emit
/// fractions of a notch many times a frame; requiring a whole notch per slot
/// and carrying the sub-slot remainder forward keeps selection tied to travel.
const SCROLL_NOTCHES_PER_SLOT: f32 = 1.0;

#[derive(Default, Copy, Clone, Debug)]
pub(super) struct PointerState {
    dx: f32,
    dy: f32,
    grabbing: bool,
    left_click: bool,
    right_click: bool,
    left_held: bool,
    right_held: bool,
    scroll_delta: f32,
    cursor_x: f32,
    cursor_y: f32,
    recenter_pending: bool,
}

impl PointerState {
    fn add_motion(&mut self, dx: f32, dy: f32) {
        self.dx += dx;
        self.dy += dy;
        self.grabbing = true;
    }

    fn set_cursor_position(&mut self, x: f32, y: f32) {
        self.cursor_x = x;
        self.cursor_y = y;
    }

    /// Set the gameplay break/use state directly — the rebindable
    /// Attack/Interact controls' landing point (bypasses screen routing).
    pub(super) fn set_gameplay_button(&mut self, button: PointerButton, down: bool) {
        self.set_button(button, down);
    }

    fn set_button(&mut self, button: PointerButton, down: bool) {
        match (button, down) {
            (PointerButton::Primary, true) => {
                self.left_click = true;
                self.left_held = true;
                self.grabbing = true;
            }
            (PointerButton::Primary, false) => {
                self.left_held = false;
            }
            (PointerButton::Secondary, true) => {
                self.right_click = true;
                self.right_held = true;
                self.grabbing = true;
            }
            (PointerButton::Secondary, false) => {
                self.right_held = false;
            }
        }
    }

    fn clear_buttons(&mut self) {
        self.left_click = false;
        self.right_click = false;
        self.left_held = false;
        self.right_held = false;
    }

    fn add_scroll_delta(&mut self, delta: f32) {
        self.scroll_delta += delta;
    }

    pub(super) fn cursor(&self) -> (f32, f32) {
        (self.cursor_x, self.cursor_y)
    }

    pub(super) fn clear_edges(&mut self) {
        self.left_click = false;
        self.right_click = false;
    }

    pub(super) fn release_for_menu(&mut self) {
        self.clear_buttons();
        self.grabbing = false;
        self.recenter_pending = true;
    }

    pub(super) fn release_buttons(&mut self) {
        self.clear_buttons();
    }

    pub(super) fn grab_for_gameplay(&mut self) {
        self.grabbing = true;
    }

    pub(super) fn recenter_if_pending(&mut self, screen_size: (u32, u32)) -> bool {
        if !self.recenter_pending {
            return false;
        }

        self.cursor_x = screen_size.0 as f32 * 0.5;
        self.cursor_y = screen_size.1 as f32 * 0.5;
        self.recenter_pending = false;
        true
    }

    /// Whole wheel notches accumulated since the last call, draining the
    /// accumulator by the notches consumed and keeping the sub-notch remainder
    /// for next frame (hi-res wheels emit fractions that sum to a notch).
    /// Positive = scroll down. Each notch fires the bindings bound to that
    /// scroll direction (hotbar next/prev by default).
    pub(super) fn take_scroll_notches(&mut self) -> i32 {
        let steps = (self.scroll_delta / SCROLL_NOTCHES_PER_SLOT).trunc();
        self.scroll_delta -= steps * SCROLL_NOTCHES_PER_SLOT;
        steps as i32
    }

    #[cfg(test)]
    fn take_scroll_step(&mut self) -> i32 {
        self.take_scroll_notches()
    }

    fn take_game_input(
        &mut self,
        input: &mut InputController,
        gameplay_enabled: bool,
    ) -> GameInput {
        let look_delta = if gameplay_enabled && self.grabbing {
            (self.dx, self.dy)
        } else {
            (0.0, 0.0)
        };
        self.dx = 0.0;
        self.dy = 0.0;

        // Hotbar stepping is a bound control now (scroll by default, keys by
        // remap): edges accumulated in the InputController, dropped outside
        // gameplay like the raw scroll accumulator.
        let steps = input.take_hotbar_steps();
        let hotbar_scroll = if gameplay_enabled {
            steps
        } else {
            self.scroll_delta = 0.0;
            0
        };

        GameInput {
            gameplay_enabled,
            movement: input.movement(),
            look_delta,
            hotbar_scroll,
            break_held: self.left_held,
            attack_clicked: self.left_click,
            place_clicked: self.right_click,
            use_held: self.right_held,
        }
    }

    #[cfg(test)]
    pub(super) fn is_grabbing(&self) -> bool {
        self.grabbing
    }
}

impl App {
    pub fn add_pointer_motion(&mut self, dx: f32, dy: f32) {
        self.pointer.add_motion(dx, dy);
    }

    pub fn set_cursor_position(&mut self, x: f32, y: f32) {
        self.pointer.set_cursor_position(x, y);
        if self.screen == super::AppScreen::Chat {
            self.chat.pointer_move(x, y, super::now_seconds());
            return;
        }
        if self.screen.client_canvas_open() {
            self.queue_client_canvas_move(x, y);
            return;
        }
        if self.doc_ui_kind().is_some() {
            self.ui
                .push_input(petramond_ui::InputEvent::PointerMove { x, y });
        }
    }

    pub fn set_pointer_button(&mut self, button: PointerButton, down: bool) {
        if self.screen == super::AppScreen::Chat {
            if button == PointerButton::Primary {
                let (x, y) = self.pointer.cursor();
                if down {
                    self.chat.pointer_down(x, y, super::now_seconds());
                } else {
                    self.chat.pointer_up();
                }
            }
            return;
        }
        self.pointer.set_button(button, down);
        if self.screen.client_canvas_open() {
            if !down {
                self.flush_client_canvas_move();
            }
            let (x, y) = self.pointer.cursor();
            let button = match button {
                PointerButton::Primary => mod_api::ClientPointerButton::Primary,
                PointerButton::Secondary => mod_api::ClientPointerButton::Secondary,
            };
            self.dispatch_client_canvas_pointer(
                if down {
                    mod_api::ClientPointerPhase::Down
                } else {
                    mod_api::ClientPointerPhase::Up
                },
                button,
                x,
                y,
            );
            return;
        }
        if self.doc_ui_kind().is_some() {
            let (x, y) = self.pointer.cursor();
            let button = match button {
                PointerButton::Primary => petramond_ui::PointerButton::Primary,
                PointerButton::Secondary => petramond_ui::PointerButton::Secondary,
            };
            self.ui.push_input(if down {
                petramond_ui::InputEvent::PointerDown {
                    x,
                    y,
                    button,
                    shift: self.modifiers.shift,
                }
            } else {
                petramond_ui::InputEvent::PointerUp { x, y, button }
            });
        }
    }

    pub fn add_scroll_delta(&mut self, delta: f32) {
        if self.remap.is_some() {
            // Remap capture: any wheel movement binds its direction.
            self.remap_capture_scroll(delta);
            return;
        }
        if self.screen == super::AppScreen::Chat {
            self.chat.scroll(delta);
            return;
        }
        if self.doc_ui_kind().is_some() {
            // One wheel notch scrolls ~20 logical px, natural direction.
            self.ui.push_input(petramond_ui::InputEvent::Scroll {
                delta: (delta * 20.0) as i32,
            });
            return;
        }
        self.pointer.add_scroll_delta(delta);
        // Whole notches fire whatever is bound to that scroll direction
        // (hotbar next/prev by default) — gameplay only, like every binding.
        if self.screen.gameplay_enabled() {
            let notches = self.pointer.take_scroll_notches();
            if notches != 0 {
                self.pulse_scroll_bindings(notches);
            }
        }
    }

    pub fn release_pointer_buttons(&mut self) {
        self.pointer.release_buttons();
        self.chat.pointer_up();
        self.audio.set_loop(None, super::now_seconds());
    }

    pub(super) fn recenter_pointer_if_pending(&mut self, screen_size: (u32, u32)) {
        if self.pointer.recenter_if_pending(screen_size) {}
    }

    pub(super) fn take_game_input(&mut self) -> GameInput {
        self.pointer
            .take_game_input(&mut self.input, self.screen.gameplay_enabled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Control;

    #[test]
    fn scroll_step_needs_a_full_notch() {
        let mut p = PointerState {
            scroll_delta: 0.4,
            ..Default::default()
        };
        assert_eq!(p.take_scroll_step(), 0);
        p.scroll_delta += 0.4;
        assert_eq!(p.take_scroll_step(), 0);
        p.scroll_delta += 0.4;
        assert_eq!(p.take_scroll_step(), 1);
        assert!((p.scroll_delta - 0.2).abs() < 1e-4);
    }

    #[test]
    fn scroll_step_is_proportional_and_signed() {
        let mut p = PointerState {
            scroll_delta: 3.0,
            ..Default::default()
        };
        assert_eq!(p.take_scroll_step(), 3);
        assert_eq!(p.scroll_delta, 0.0);

        p.scroll_delta = -2.5;
        assert_eq!(p.take_scroll_step(), -2);
        assert!((p.scroll_delta + 0.5).abs() < 1e-4);
    }

    #[test]
    fn scroll_step_carries_remainder_across_frames() {
        let mut p = PointerState::default();
        let mut steps = 0;
        for _ in 0..25 {
            p.scroll_delta += 0.1;
            steps += p.take_scroll_step();
        }
        assert_eq!(steps, 2);
        assert!((p.scroll_delta - 0.5).abs() < 1e-4);
    }

    #[test]
    fn game_input_gates_look_and_scroll_when_gameplay_is_disabled() {
        let mut input = InputController::default();
        input.set_control(Control::MoveForward, true);
        let mut p = PointerState {
            dx: 5.0,
            dy: -2.0,
            grabbing: true,
            left_click: true,
            right_click: true,
            left_held: true,
            scroll_delta: 2.0,
            ..Default::default()
        };

        let game_input = p.take_game_input(&mut input, false);

        assert!(!game_input.gameplay_enabled);
        assert!(game_input.movement.forward);
        assert_eq!(game_input.look_delta, (0.0, 0.0));
        assert_eq!(game_input.hotbar_scroll, 0);
        assert!(game_input.break_held);
        assert!(game_input.attack_clicked);
        assert!(game_input.place_clicked);
        assert_eq!(p.scroll_delta, 0.0);

        let game_input = p.take_game_input(&mut input, true);
        assert_eq!(game_input.look_delta, (0.0, 0.0));
    }

    #[test]
    fn button_edges_and_held_state_feed_game_input() {
        let mut input = InputController::default();
        let mut p = PointerState::default();

        p.set_button(PointerButton::Primary, true);
        p.set_button(PointerButton::Primary, false);
        let game_input = p.take_game_input(&mut input, true);
        assert!(p.is_grabbing());
        assert!(!game_input.break_held);
        assert!(game_input.attack_clicked);
        assert!(!game_input.place_clicked);

        p.clear_edges();
        p.set_button(PointerButton::Secondary, true);
        p.set_button(PointerButton::Secondary, false);
        let game_input = p.take_game_input(&mut input, true);
        assert!(game_input.place_clicked);
        assert!(!game_input.attack_clicked);
    }
}
