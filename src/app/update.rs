use super::{now_seconds, App};
use crate::render::Renderer;

impl App {
    /// Advance input and the simulation for this frame. The host calls this once per
    /// frame wake and then draws; [`Game::tick`](crate::game::Game::tick)'s fixed-step
    /// accumulator holds the world at 20 TPS regardless of frame rate.
    pub fn update(&mut self, renderer: &Renderer) {
        self.update_in_viewport(renderer.ui_viewport());
    }

    /// [`update`](Self::update) behind the renderer handoff — the whole frame
    /// advance against a bare screen size, so tests can drive real frames
    /// headlessly.
    #[cfg(test)]
    pub(crate) fn update_frame(&mut self, screen_size: (u32, u32)) {
        self.update_in_viewport(crate::gui::UiViewport::unversioned(screen_size));
    }

    fn update_in_viewport(&mut self, viewport: crate::gui::UiViewport) {
        let screen_size = viewport.size;
        self.ui.set_viewport_generation(viewport.generation);
        let now = now_seconds();
        let dt = (now - self.last) as f32;
        self.last = now;

        self.recenter_pointer_if_pending(screen_size);

        self.drive_client_mod_frame(dt, screen_size);

        // Document-backed SHELL screens run their whole UI frame here (input
        // → events → controller) and skip the simulation entirely; render
        // only hands the built draw list over. The legacy click routers must
        // not also fire on their invisible layouts.
        let pause_runs_sim = self.multiplayer_pause_runs_sim();
        if let Some(kind) = self.doc_shell_kind() {
            self.audio.set_loop(None, now);
            self.pointer.clear_edges();
            self.drive_doc_ui(kind, screen_size, now);
            if !pause_runs_sim {
                // Shell screens (pause menu) skip Game::tick, but the server
                // thread keeps streaming: keep consuming its output so nothing
                // backs up and resume is instant (multiplayer Phase D).
                self.pump_network_and_watch();
                return;
            }
            // Multiplayer pause menu: fall through to the simulation below.
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
        else if self.screen.client_ui_open() {
            if let Some(kind) = self.doc_ui_kind() {
                self.drive_client_doc_ui(kind, screen_size, now);
            }
            self.pointer.clear_edges();
        } else if let Some(kind) = self.doc_ui_kind() {
            self.drive_doc_menu(kind, screen_size, now);
            self.pointer.clear_edges();
        }

        if (self.screen.shell_open() && !pause_runs_sim) || self.game.is_none() {
            self.audio.set_loop(None, now);
            self.pointer.clear_edges();
            // Same as the doc-shell path above: keep draining the server.
            self.pump_network_and_watch();
            return;
        }

        let game_input = self.take_game_input();
        let events = self
            .game
            .as_mut()
            .expect("game exists after shell/no-game guard")
            .tick(dt, &game_input);
        self.adopt_chat_lines(now);
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

    /// The pause menu is up but pausing is INEFFECTIVE, so the client must
    /// keep simulating behind it: the server permanently ignores `Pause` once
    /// it has been opened to LAN (`lan_ever_opened` — mirrored here by
    /// `lan_port`, which lives exactly as long as the session), and a remote
    /// client never pauses the shared server at all. Freezing only this
    /// client would stop its `PlayerUpdate`s and per-frame systems (entity
    /// push, interpolation) while the world runs on — a statue that can't be
    /// jostled. Gameplay INPUT stays disabled on the Pause screen regardless
    /// (`take_game_input`).
    fn multiplayer_pause_runs_sim(&self) -> bool {
        self.screen == super::AppScreen::Pause
            && self
                .game
                .as_ref()
                .is_some_and(|g| g.is_remote() || self.lan_port.is_some())
    }

    /// Drain the server while `Game::tick` is suppressed (shell screens over
    /// a live game — the pause menu), and still notice a lost connection:
    /// ticks surface it through `GameEvents`, but here nobody assembles them.
    fn pump_network_and_watch(&mut self) {
        let lost = if let Some(game) = self.game.as_mut() {
            game.pump_network();
            game.take_connection_lost()
        } else {
            None
        };
        self.adopt_chat_lines(super::now_seconds());
        if let Some(reason) = lost {
            self.enter_connection_lost(reason);
        }
    }

    fn adopt_chat_lines(&mut self, now: f64) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        for line in game.take_chat_lines() {
            self.chat.push(line, now);
        }
    }
}
