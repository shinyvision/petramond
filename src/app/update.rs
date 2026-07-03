use super::{now_seconds, App};
use crate::render::Renderer;

impl App {
    /// Advance input and the simulation for this frame. The host calls this once per
    /// frame wake and then draws; [`Game::tick`](crate::game::Game::tick)'s fixed-step
    /// accumulator holds the world at 20 TPS regardless of frame rate.
    pub fn update(&mut self, renderer: &Renderer) {
        let now = now_seconds();
        let dt = (now - self.last) as f32;
        self.last = now;

        let screen_size = renderer.screen_size();
        self.recenter_pointer_if_pending(screen_size);

        // Route inventory clicks before reading game input, so a right-click
        // consumed by the open inventory never also fires block placement.
        if self.pointer.left_clicked() && self.route_screen_click(screen_size, now) {
            self.pointer.clear_left_click();
        }
        if self.pointer.right_clicked() && self.route_screen_right_click(screen_size, now) {
            self.pointer.clear_right_click();
        }
        if self.pointer.left_clicked() && self.route_shell_click(screen_size, now) {
            self.pointer.clear_left_click();
        }
        if self.pointer.left_held() {
            self.route_shell_drag(screen_size, now);
        } else {
            self.clear_shell_drag();
        }

        if self.screen.shell_open() || self.game.is_none() {
            self.audio.set_loop(None, now);
            self.pointer.clear_edges();
            return;
        }

        let game_input = self.take_game_input();
        let events = self
            .game
            .as_mut()
            .expect("game exists after shell/no-game guard")
            .tick(dt, &game_input);
        self.handle_open_screen_events(&events);
        let mining_block = (self.screen.gameplay_enabled() && game_input.break_held)
            .then(|| {
                self.game
                    .as_ref()
                    .expect("game exists after shell/no-game guard")
                    .client_frame(now)
                    .held_item
                    .mining_block
            })
            .flatten();
        self.play_game_event_sounds(&events, mining_block, now);
        self.pointer.clear_edges();
        self.latch_game_event_hand_triggers(&events);
    }
}
