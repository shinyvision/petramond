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

        // Document-backed SHELL screens run their whole UI frame here (input
        // → events → controller) and skip the simulation entirely; render
        // only hands the built draw list over. The legacy click routers must
        // not also fire on their invisible layouts.
        if let Some(kind) = self.doc_shell_kind() {
            self.audio.set_loop(None, now);
            self.pointer.clear_edges();
            self.drive_doc_ui(kind, screen_size, now);
            return;
        }

        // Gameplay OVERLAYS (sleep fade, death screen) drive the document like
        // a shell screen — their buttons dispatch to controllers — but fall
        // through to the simulation below: the sleep timer and respawn are
        // tick-owned, so the world must keep ticking behind them.
        if let Some(kind) = self.doc_overlay_kind() {
            self.drive_doc_ui(kind, screen_size, now);
            self.pointer.clear_edges();
        }
        // Document-backed game MENUS (mod GUIs, containers) drive their UI
        // frame here too — slot/widget clicks latch to the tick through the
        // document runtime (there is no other click route) — and the
        // simulation continues below. Clearing the pointer edges keeps a
        // menu-consumed click from also firing block break/placement.
        else if let Some(kind) = self.doc_ui_kind() {
            self.drive_doc_menu(kind, screen_size, now);
            self.pointer.clear_edges();
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
