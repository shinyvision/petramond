use super::{now_seconds, App};
use petramond_render::Renderer;

impl App {
    /// Advance input and the simulation for this frame. The host calls this once per
    /// frame wake and then draws; the SERVER thread owns the fixed-step accumulator
    /// that holds the world at 20 TPS (`src/server/handle.rs`) —
    /// [`Game::tick`](crate::game::Game::tick) only ships this frame's messages and
    /// drains what the server produced.
    pub fn update(&mut self, renderer: &Renderer) {
        self.update_in_viewport(renderer.ui_viewport());
    }

    /// Whether a document-backed GAME menu is up with a live session behind
    /// it — the host paces these at the full gameplay cadence: the panel's
    /// bound state answers the server, and a menu frame cap would tax every
    /// round trip twice (input sampling and drain-to-present).
    pub fn game_menu_open(&self) -> bool {
        self.game.is_some() && self.game_menu_kind().is_some()
    }

    /// Whether a client-mod modal canvas (e.g. the world map) is on screen.
    /// These pace at the full fps cap, not the menu cap: they are live
    /// interactive surfaces (drag panning, budgeted progressive fills), and
    /// menu-rate framing both halves their per-frame work budgets and makes
    /// dragging feel choppy.
    pub fn client_canvas_screen(&self) -> bool {
        self.screen.client_canvas_open()
    }

    /// The document solved as a GAME MENU this frame: a container / machine
    /// panel or a gameplay overlay — anything driven with the simulation still
    /// running behind it. `None` for SHELL documents, which their own branch
    /// populates and drives, and for client-mod GUIs, which have their own
    /// drive route. The single spelling of the condition: the menu is solved
    /// twice per frame and paced by [`game_menu_open`](Self::game_menu_open),
    /// and those must not disagree about what is on screen.
    fn game_menu_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        if self.screen.client_ui_open() || self.doc_shell_kind().is_some() {
            return None;
        }
        self.doc_ui_kind()
    }

    /// [`update`](Self::update) behind the renderer handoff — the whole frame
    /// advance against a bare screen size, so tests can drive real frames
    /// headlessly.
    #[cfg(test)]
    pub fn update_frame(&mut self, screen_size: (u32, u32)) {
        self.update_in_viewport(petramond::gui::UiViewport::unversioned(screen_size));
    }

    fn update_in_viewport(&mut self, viewport: petramond::gui::UiViewport) {
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
        // The world's sounds freeze exactly when the world does: on the two
        // early returns below that skip `Game::tick` (a shell screen over a
        // live game whose pause is effective). A multiplayer pause menu runs
        // the sim on, so the cart that passes behind it stays audible.
        let world_frozen = self.game.is_some()
            && !pause_runs_sim
            && (self.doc_shell_kind().is_some() || self.screen.shell_open());
        self.audio.set_spatial_paused(world_frozen);
        if let Some(kind) = self.doc_shell_kind() {
            self.audio.set_loop(None, now);
            self.pointer.clear_edges();
            self.drive_doc_ui(kind, screen_size, now);
            if !pause_runs_sim {
                // Shell screens (pause menu) skip Game::tick, but the server
                // thread keeps streaming: keep consuming its output so nothing
                // backs up and resume is instant.
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
        } else if self.doc_ui_kind().is_some() {
            // A shell document is the SHELL branch's to drive, and
            // `game_menu_kind` withholds it here: the multiplayer
            // fall-through reaches this arm, and the shell doc just driven
            // may have flipped the screen to ANOTHER shell doc. Driving that
            // as a menu would stamp a frame its controller never populated —
            // presenting one frame of unbound state (the options title
            // backdrop over a live game). The edges clear either way: a
            // document is up, so the click was never the world's.
            if let Some(kind) = self.game_menu_kind() {
                self.drive_doc_menu(kind, screen_size, now);
            }
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
        // The tick above just drained the server: an open menu's read model
        // (slot mirrors, mod gui_state) may have moved. RE-SOLVE the panel so
        // THIS frame presents this tick's answer — solved only before the
        // drain, every server response would cost one extra whole frame on
        // screen. The input queue was consumed by the first solve, so this
        // pass is pure presentation: no event fires twice.
        if let Some(kind) = self.game_menu_kind() {
            self.drive_doc_menu(kind, screen_size, now);
        }
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
        (self.screen == super::AppScreen::Pause || self.screen.options_open())
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
