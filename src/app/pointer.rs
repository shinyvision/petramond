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
                self.grabbing = true;
            }
            (PointerButton::Secondary, false) => {}
        }
    }

    fn clear_buttons(&mut self) {
        self.left_click = false;
        self.right_click = false;
        self.left_held = false;
    }

    fn add_scroll_delta(&mut self, delta: f32) {
        self.scroll_delta += delta;
    }

    pub(super) fn cursor(&self) -> (f32, f32) {
        (self.cursor_x, self.cursor_y)
    }

    pub(super) fn left_clicked(&self) -> bool {
        self.left_click
    }

    pub(super) fn right_clicked(&self) -> bool {
        self.right_click
    }

    pub(super) fn clear_left_click(&mut self) {
        self.left_click = false;
    }

    pub(super) fn clear_right_click(&mut self) {
        self.right_click = false;
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

    /// Whole hotbar slots to move this frame, draining the accumulator by the
    /// notches consumed and keeping the sub-slot remainder for next frame.
    fn take_scroll_step(&mut self) -> i32 {
        let steps = (self.scroll_delta / SCROLL_NOTCHES_PER_SLOT).trunc();
        self.scroll_delta -= steps * SCROLL_NOTCHES_PER_SLOT;
        steps as i32
    }

    fn take_game_input(&mut self, input: &InputController, gameplay_enabled: bool) -> GameInput {
        let look_delta = if gameplay_enabled && self.grabbing {
            (self.dx, self.dy)
        } else {
            (0.0, 0.0)
        };
        self.dx = 0.0;
        self.dy = 0.0;

        let hotbar_scroll = if gameplay_enabled {
            self.take_scroll_step()
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
    }

    pub fn set_pointer_button(&mut self, button: PointerButton, down: bool) {
        self.pointer.set_button(button, down);
    }

    pub fn add_scroll_delta(&mut self, delta: f32) {
        if !self.adjust_world_scroll(delta) {
            self.pointer.add_scroll_delta(delta);
        }
    }

    pub fn release_pointer_buttons(&mut self) {
        self.pointer.release_buttons();
        self.audio.set_loop(None, super::now_seconds());
    }

    pub(super) fn recenter_pointer_if_pending(&mut self, screen_size: (u32, u32)) {
        if self.pointer.recenter_if_pending(screen_size) {}
    }

    pub(super) fn take_game_input(&mut self) -> GameInput {
        self.pointer
            .take_game_input(&self.input, self.screen.gameplay_enabled())
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

        let game_input = p.take_game_input(&input, false);

        assert!(!game_input.gameplay_enabled);
        assert!(game_input.movement.forward);
        assert_eq!(game_input.look_delta, (0.0, 0.0));
        assert_eq!(game_input.hotbar_scroll, 0);
        assert!(game_input.break_held);
        assert!(game_input.attack_clicked);
        assert!(game_input.place_clicked);
        assert_eq!(p.scroll_delta, 0.0);

        let game_input = p.take_game_input(&input, true);
        assert_eq!(game_input.look_delta, (0.0, 0.0));
    }

    #[test]
    fn button_edges_and_held_state_feed_game_input() {
        let input = InputController::default();
        let mut p = PointerState::default();

        p.set_button(PointerButton::Primary, true);
        p.set_button(PointerButton::Primary, false);
        let game_input = p.take_game_input(&input, true);
        assert!(p.is_grabbing());
        assert!(!game_input.break_held);
        assert!(game_input.attack_clicked);
        assert!(!game_input.place_clicked);

        p.clear_edges();
        p.set_button(PointerButton::Secondary, true);
        p.set_button(PointerButton::Secondary, false);
        let game_input = p.take_game_input(&input, true);
        assert!(game_input.place_clicked);
        assert!(!game_input.attack_clicked);
    }
}
