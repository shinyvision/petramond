use super::screen::ScreenRole;
use super::{App, AppScreen};
use petramond_render::Renderer;

impl App {
    pub fn update(&mut self, renderer: &mut Renderer) {
        self.update_in_viewport(renderer.window_ui_viewport(), Some(renderer));
    }

    pub fn game_menu_open(&self) -> bool {
        self.session.is_some() && self.game_menu_kind().is_some()
    }

    pub fn client_canvas_screen(&self) -> bool {
        self.screen.client_canvas_open()
    }

    fn game_menu_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        match self.screen.role() {
            ScreenRole::GameMenu | ScreenRole::Overlay => self.doc_ui_kind(),
            _ => None,
        }
    }

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
        let step = self.step_media_clock();
        let wall = super::now_seconds();
        let now = self.media.clock.now(wall);
        let dt = step.unwrap_or(now - self.last) as f32;
        self.last = now;
        let wall_dt = (wall - self.last_wall) as f32;
        self.last_wall = wall;

        self.recenter_pointer_if_pending(screen_size);
        self.poll_content();

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
                self.pump_network_and_watch();
                return;
            }
        }

        match self.screen.role() {
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
            self.renderer_world_clear_pending = true;
            self.sound.clear_spatial();
        } else if events.presented_time_jumped {
            self.renderer_moment_clear_pending = true;
            self.sound.clear_spatial();
        }
        self.adopt_chat_lines(now);
        self.handle_open_screen_events(&events);
        self.open_requested_schematic_library();
        if let Some(kind) = self.game_menu_kind() {
            self.drive_doc_menu(kind, screen_size, now);
        }
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

    /// The pause menu is up but pausing does nothing, so keep simulating. The server ignores
    /// `Pause` once opened to LAN (mirrored here by `lan_port`), and a remote client never pauses
    /// the shared server. Freezing just this client would stop its `PlayerUpdate`s while the world
    /// runs on. Gameplay input stays off on the Pause screen either way.
    fn multiplayer_pause_runs_sim(&self) -> bool {
        self.pause_open()
            && self
                .session
                .as_ref()
                .is_some_and(|s| s.game.is_remote() || s.lan_port.is_some())
    }

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
