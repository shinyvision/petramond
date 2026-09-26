//! App-level shell actions: entering and leaving game sessions (start,
//! adopt, pause/resume, LAN, save-and-quit, disconnect, connection loss) and
//! the text-input hooks that forward platform keyboard events into the
//! GUI-document runtime. The title flow's own state and world I/O live in
//! `super::shell_state`.

use super::session::Session;
use super::{now_seconds, App, AppScreen};
use petramond_input::controls::{text_shortcut_from_key_code, TextKey, TextShortcut};
use petramond_render::camera::Camera;

impl App {
    /// Forward a text-editing key to the document UI. Returns whether it was
    /// consumed (false when no document screen is active).
    pub fn handle_text_key(&mut self, key: TextKey) -> bool {
        if self.screen == AppScreen::Chat {
            let now = now_seconds();
            match key {
                TextKey::Enter => {
                    if let Some(Session { game, chat, .. }) = self.session.as_mut() {
                        if let Some(text) = chat.submit_or_close(now) {
                            game.send_chat(text);
                        }
                    }
                    self.set_screen(AppScreen::Game);
                }
                _ => {
                    if let Some(session) = self.session.as_mut() {
                        session.chat.edit_key(
                            nav_key_from_text_key(key),
                            self.controls.modifiers.shift,
                            self.controls.modifiers.ctrl,
                            None,
                            now,
                        );
                    }
                }
            }
            return true;
        }
        if self.doc_ui_kind().is_none() {
            return false;
        }
        self.ui.push_input(petramond_ui::InputEvent::Key {
            key: nav_key_from_text_key(key),
            shift: self.controls.modifiers.shift,
            ctrl: self.controls.modifiers.ctrl,
        });
        true
    }

    /// Resolve a physical key + tracked modifiers into a text shortcut and
    /// forward it. Clipboard access lives inside the document UI (`AppUi`
    /// owns its own clipboard), so no host clipboard is threaded through.
    pub fn handle_text_shortcut_code(&mut self, code: petramond_input::keycode::KeyCode) -> bool {
        let Some(shortcut) = text_shortcut_from_key_code(code, self.controls.modifiers) else {
            return false;
        };
        self.handle_text_shortcut(shortcut)
    }

    pub fn handle_text_shortcut(&mut self, shortcut: TextShortcut) -> bool {
        if self.screen == AppScreen::Chat {
            let key = nav_key_from_shortcut(shortcut);
            let now = now_seconds();
            let clipboard = self.ui.clipboard_mut();
            if let Some(session) = self.session.as_mut() {
                session
                    .chat
                    .edit_key(key, false, false, Some(clipboard), now);
            }
            return true;
        }
        if self.doc_ui_kind().is_none() {
            return false;
        }
        self.ui.push_input(petramond_ui::InputEvent::Key {
            key: nav_key_from_shortcut(shortcut),
            shift: false,
            ctrl: false,
        });
        true
    }

    pub fn handle_text_input(&mut self, text: &str) -> bool {
        if self.screen == AppScreen::Chat {
            if let Some(session) = self.session.as_mut() {
                session.chat.insert_text(text, now_seconds());
            }
            return true;
        }
        if self.doc_ui_kind().is_none() {
            return false;
        }
        for ch in text.chars() {
            self.ui.push_input(petramond_ui::InputEvent::Char { ch });
        }
        true
    }

    pub fn take_quit_requested(&mut self) -> bool {
        std::mem::take(&mut self.quit_requested)
    }

    pub(super) fn open_pause(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        // Pause is a protocol message: the server
        // thread keeps streaming/autosaving but skips the fixed ticks. The
        // screen switch below is what stops App::update calling Game::tick
        // (it still pumps the network — see update.rs).
        session.game.set_paused(true);
        self.set_screen(AppScreen::Pause);
        self.sound.stop_mining_loop(now_seconds());
    }

    pub(super) fn resume_game(&mut self) {
        let Some(session) = self.session.as_mut() else {
            self.set_screen(AppScreen::Title);
            return;
        };
        session.game.set_paused(false);
        self.set_screen(AppScreen::Game);
    }

    /// The pause menu's Open to LAN: bind the default port into the running
    /// HOST server. Success shows the port label; failure shows inline.
    pub(super) fn open_lan(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let port = petramond::net::DEFAULT_PORT;
        match session.game.open_to_lan(port) {
            Ok(bound) => {
                session.lan_port = Some(bound);
                session.lan_error = None;
            }
            Err(e) => session.lan_error = Some(format!("Couldn't open port {port}: {e}")),
        }
    }

    /// Save and Quit (a HOST session): ending the session joins the server
    /// thread, which saves everything before exiting.
    pub(super) fn save_and_quit_to_title(&mut self) {
        debug_assert!(
            self.session.as_ref().is_none_or(|s| !s.game.is_remote()),
            "save-and-quit is a HOST action; remote sessions disconnect"
        );
        self.end_session(AppScreen::Title);
    }

    /// Leave a REMOTE session (the pause menu's Disconnect): dropping the
    /// handle's sender makes the connection writer flush a farewell
    /// `Disconnect`; the server saves our player on its leave path. Nothing
    /// to save locally.
    pub(super) fn disconnect_to_title(&mut self) {
        debug_assert!(
            self.session.as_ref().is_some_and(|s| s.game.is_remote()),
            "Disconnect is a remote-session action"
        );
        self.end_session(AppScreen::Title);
    }

    /// The involuntary exit: the server became unreachable (host thread
    /// crash, remote server close / connection loss). NO save — a crashed
    /// host has no server thread left to ask (its shutdown is a no-op join),
    /// and a remote server saves autonomously. Lands on the Disconnected
    /// screen with the reason.
    pub(super) fn enter_connection_lost(&mut self, reason: String) {
        self.shell.set_disconnect_message(reason);
        self.end_session(AppScreen::ConnectionLost);
    }

    /// End the live session — the one teardown every quit, disconnect and
    /// connection-loss path shares — and land on `next`. Dropping the
    /// [`Session`] takes every session-scoped value with it; what remains
    /// here is the app-lifetime side reacting: the section cache parked for
    /// a reconnect, the engine's voices, the action table, the renderer's
    /// world, the world list.
    fn end_session(&mut self, next: AppScreen) {
        if let Some(session) = self.session.take() {
            self.retained_section_cache = Some(session.end());
        }
        self.sound.end_session(now_seconds());
        self.rebuild_action_table();
        self.renderer_world_clear_pending = true;
        self.shell.refresh_worlds();
        self.set_screen(next);
    }

    pub(super) fn play_selected_world(&mut self) {
        let Some(world) = self.shell.selected_world_info().cloned() else {
            return;
        };
        let seed = petramond::save::random_seed();
        self.start_game(&world.dir_name, seed);
    }

    /// Open (or create) the world saved under `world_dir_name` —
    /// `WorldInfo::dir_name`, NOT the display name: renames change only the
    /// display name, so opening by name would silently start a fresh world.
    pub fn start_game(&mut self, world_dir_name: &str, seed: u32) {
        let cam = Camera::new(
            petramond_math::world_pos::WorldPos::new(8.0, 90.0, 8.0),
            self.shell_camera.aspect.max(0.01),
        );
        self.adopt_game(crate::game::Game::new(
            cam,
            world_dir_name,
            seed,
            self.render_dist,
        ));
        // The per-world auto-LAN rule: host straight from load. Failure shows
        // inline on the pause menu like a manual Open to LAN.
        if petramond::save::read_world_settings(world_dir_name).auto_open_lan {
            self.open_lan();
        }
    }

    /// Install a freshly-built game session and enter gameplay — the shared
    /// tail of `start_game` (which builds the session, spawning the server
    /// thread) and the test fixtures (which build a loopback-piped session).
    /// Sessions are adopted from the title flow, after the previous one
    /// ended.
    pub fn adopt_game(&mut self, game: crate::game::Game) {
        debug_assert!(
            self.session.is_none(),
            "a session is adopted only once the previous one has ended"
        );
        // A world saved while dead (quit from the death screen, or a crash)
        // reopens ON the death screen — a 0-health player must never resume
        // walking around.
        let dead = game.player_health().is_some_and(|h| h.current == 0);
        self.session = Some(Session::new(game));
        self.apply_particles();
        self.rebuild_action_table();
        self.renderer_world_clear_pending = false;
        self.set_screen(if dead {
            AppScreen::Dead
        } else {
            AppScreen::Game
        });
    }
}

fn nav_key_from_text_key(key: TextKey) -> petramond_ui::NavKey {
    match key {
        TextKey::Backspace => petramond_ui::NavKey::Backspace,
        TextKey::Delete => petramond_ui::NavKey::Delete,
        TextKey::Enter => petramond_ui::NavKey::Enter,
        TextKey::Tab => petramond_ui::NavKey::Tab,
        TextKey::ArrowLeft => petramond_ui::NavKey::Left,
        TextKey::ArrowRight => petramond_ui::NavKey::Right,
        TextKey::ArrowUp => petramond_ui::NavKey::Up,
        TextKey::ArrowDown => petramond_ui::NavKey::Down,
        TextKey::Home => petramond_ui::NavKey::Home,
        TextKey::End => petramond_ui::NavKey::End,
    }
}

fn nav_key_from_shortcut(shortcut: TextShortcut) -> petramond_ui::NavKey {
    match shortcut {
        TextShortcut::SelectAll => petramond_ui::NavKey::SelectAll,
        TextShortcut::Cut => petramond_ui::NavKey::Cut,
        TextShortcut::Copy => petramond_ui::NavKey::Copy,
        TextShortcut::Paste => petramond_ui::NavKey::Paste,
    }
}
