use super::screen::ScreenRole;
use super::{App, AppScreen};
use petramond_render::Renderer;

impl App {
    /// Advance input and the simulation for this frame. The host calls this once per
    /// frame wake and then draws; the SERVER thread owns the fixed-step accumulator
    /// that holds the world at 20 TPS (`src/server/handle.rs`) —
    /// [`Game::tick`](crate::game::Game::tick) only ships this frame's messages and
    /// drains what the server produced.
    pub fn update(&mut self, renderer: &mut Renderer) {
        self.update_in_viewport(renderer.window_ui_viewport(), Some(renderer));
    }

    /// Whether a document-backed GAME menu is up with a live session behind
    /// it — the host paces these at the full gameplay cadence: the panel's
    /// bound state answers the server, and a menu frame cap would tax every
    /// round trip twice (input sampling and drain-to-present).
    pub fn game_menu_open(&self) -> bool {
        self.session.is_some() && self.game_menu_kind().is_some()
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
        match self.screen.role() {
            ScreenRole::GameMenu | ScreenRole::Overlay => self.doc_ui_kind(),
            _ => None,
        }
    }

    /// [`update`](Self::update) behind the renderer handoff — the whole frame
    /// advance against a bare screen size, so tests can drive real frames
    /// headlessly.
    #[cfg(test)]
    pub fn update_frame(&mut self, screen_size: (u32, u32)) {
        self.update_in_viewport(petramond::gui::UiViewport::unversioned(screen_size), None);
    }

    fn update_in_viewport(
        &mut self,
        viewport: petramond::gui::UiViewport,
        renderer: Option<&mut Renderer>,
    ) {
        let screen_size = viewport.size;
        self.ui.set_viewport_generation(viewport.generation);
        // Stepped, the clock says how far this frame moves time: its queued
        // advances, or nothing for a held frame.
        let step = self.step_media_clock();
        let wall = super::now_seconds();
        let now = self.media.clock.now(wall);
        let dt = step.unwrap_or(now - self.last) as f32;
        self.last = now;
        let wall_dt = (wall - self.last_wall) as f32;
        self.last_wall = wall;

        self.recenter_pointer_if_pending(screen_size);
        self.poll_content();

        // Shell screens freeze the world — unless the pause is ineffective
        // (a multiplayer pause menu runs the sim on, so the cart that passes
        // behind it stays audible).
        let pause_runs_sim = self.multiplayer_pause_runs_sim();
        let world_frozen =
            self.session.is_some() && !pause_runs_sim && !self.screen.role().runs_sim();
        self.drive_client_mod_frame(
            dt,
            wall_dt,
            viewport,
            world_frozen || self.session.is_none(),
        );
        if let Some(renderer) = renderer {
            self.drive_media(renderer, f64::from(wall_dt));
            self.drive_frame_size(renderer);
        }
        // The soundtrack is driven HERE, above every screen's early return:
        // music belongs to the SESSION, not to whatever screen is open over
        // it — an inventory or a chest must never stop it. A frozen world lets
        // the current track finish but schedules no new one, and its spatial
        // sounds freeze exactly when it does. A presentation is a recording
        // on screen, not a session: the soundtrack is a player's, and a
        // video's music is its maker's.
        let in_session = self
            .session
            .as_ref()
            .is_some_and(|session| !session.game.in_presentation());
        self.sound.update_session(in_session, world_frozen, dt);

        // Document-backed SHELL screens run their whole UI frame here (input
        // → events → controller) and skip the simulation entirely; render
        // only hands the built draw list over. The legacy click routers must
        // not also fire on their invisible layouts.
        if let Some(kind) = self.doc_shell_kind() {
            self.sound.stop_mining_loop(now);
            self.controls.pointer.clear_edges();
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

        // Every other document-backed screen drives its UI frame here, by the
        // role of the screen that is up NOW — the shell document just driven
        // may have switched screens. A document being up means the click was
        // never the world's, so the pointer edges clear either way.
        match self.screen.role() {
            // Gameplay OVERLAYS (sleep fade, death screen) drive the document
            // like a shell screen — their buttons dispatch to controllers —
            // but the simulation continues below: the sleep timer and respawn
            // are tick-owned.
            ScreenRole::Overlay => {
                if let Some(kind) = self.doc_overlay_kind() {
                    self.drive_doc_ui(kind, screen_size, now);
                    self.controls.pointer.clear_edges();
                }
            }
            ScreenRole::ClientDoc => {
                if self.screen == AppScreen::Schematics {
                    self.drive_schematics_screen(screen_size, now);
                } else if let Some(kind) = self.doc_ui_kind() {
                    self.drive_client_doc_ui(kind, screen_size, now);
                }
                self.controls.pointer.clear_edges();
            }
            // Document-backed game MENUS (mod GUIs, containers): slot/widget
            // clicks latch to the tick through the document runtime (there is
            // no other click route), and the simulation continues below.
            ScreenRole::GameMenu => {
                if let Some(kind) = self.game_menu_kind() {
                    self.drive_doc_menu(kind, screen_size, now);
                    self.controls.pointer.clear_edges();
                }
            }
            // A shell document is the SHELL branch's to drive. The
            // multiplayer fall-through lands here, and the shell doc just
            // driven may have flipped the screen to ANOTHER shell doc; driving
            // that as a menu would stamp a frame its controller never
            // populated — presenting one frame of unbound state (the options
            // title backdrop over a live game).
            ScreenRole::Shell => {
                if self.doc_ui_kind().is_some() {
                    self.controls.pointer.clear_edges();
                }
            }
            ScreenRole::Gameplay | ScreenRole::Chat | ScreenRole::Canvas => {}
        }

        if (self.screen.shell_open() && !pause_runs_sim) || self.session.is_none() {
            self.sound.stop_mining_loop(now);
            self.controls.pointer.clear_edges();
            // Same as the doc-shell path above: keep draining the server.
            self.pump_network_and_watch();
            return;
        }

        let game_input = self.take_game_input();
        let events = self
            .session
            .as_mut()
            .expect("session exists after shell/no-session guard")
            .game
            .tick(dt, &game_input);
        if self
            .session
            .as_mut()
            .is_some_and(|session| session.game.take_presentation_closed())
        {
            self.end_presentation();
            return;
        }
        if events.presented_world_replaced {
            // A presentation swapped the presented world: what the
            // renderer holds and every playing spatial sound belong to the
            // world that left.
            self.renderer_world_clear_pending = true;
            self.sound.clear_spatial();
        } else if events.presented_time_jumped {
            // A seek: the terrain on screen stays, but what sounds and moves
            // belongs to the moment that was left. The loops still playing at
            // the target restart with this frame's events.
            self.renderer_moment_clear_pending = true;
            self.sound.clear_spatial();
        }
        self.adopt_chat_lines(now);
        self.handle_open_screen_events(&events);
        self.open_requested_schematic_library();
        // The tick above just drained the server: an open menu's read model
        // (slot mirrors, mod gui_state) may have moved. RE-SOLVE the panel so
        // THIS frame presents this tick's answer — solved only before the
        // drain, every server response would cost one extra whole frame on
        // screen. The input queue was consumed by the first solve, so this
        // pass is pure presentation: no event fires twice.
        if let Some(kind) = self.game_menu_kind() {
            self.drive_doc_menu(kind, screen_size, now);
        }
        // Only the dug block feeds the dig loop: read it alone rather than
        // assembling a whole client frame.
        let mining_block = self
            .session
            .as_ref()
            .expect("session exists after shell/no-session guard")
            .game
            .dig_loop_block(self.screen.gameplay_enabled() && game_input.break_held);
        self.play_game_event_sounds(&events, mining_block, now);
        self.controls.pointer.clear_edges();
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
    /// (`take_game_input`). The Options flow pushed over the pause menu keeps
    /// it in force.
    fn multiplayer_pause_runs_sim(&self) -> bool {
        self.pause_open()
            && self
                .session
                .as_ref()
                .is_some_and(|s| s.game.is_remote() || s.lan_port.is_some())
    }

    /// Drain the server while `Game::tick` is suppressed (shell screens over
    /// a live game — the pause menu), and still notice a lost connection:
    /// ticks surface it through `GameEvents`, but here nobody assembles them.
    fn pump_network_and_watch(&mut self) {
        let lost = if let Some(session) = self.session.as_mut() {
            session.game.pump_network();
            session.game.take_connection_lost()
        } else {
            None
        };
        self.adopt_chat_lines(self.now());
        if let Some(reason) = lost {
            self.enter_connection_lost(reason);
        }
    }

    fn adopt_chat_lines(&mut self, now: f64) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        for line in session.game.take_chat_lines() {
            session.chat.push(line, now);
        }
    }
}
