use super::session::Session;
use super::{App, AppScreen};
use petramond_input::controls::{text_shortcut_from_key_code, TextKey, TextShortcut};
use petramond_render::camera::Camera;

impl App {
    pub fn handle_text_key(&mut self, key: TextKey) -> bool {
        if self.screen == AppScreen::Chat {
            let now = self.now();
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

    pub fn handle_text_shortcut_code(&mut self, code: petramond_input::keycode::KeyCode) -> bool {
        let Some(shortcut) = text_shortcut_from_key_code(code, self.controls.modifiers) else {
            return false;
        };
        self.handle_text_shortcut(shortcut)
    }

    pub fn handle_text_shortcut(&mut self, shortcut: TextShortcut) -> bool {
        if self.screen == AppScreen::Chat {
            let key = nav_key_from_shortcut(shortcut);
            let now = self.now();
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
        let chord = matches!(shortcut, TextShortcut::Chord(_));
        self.ui.push_input(petramond_ui::InputEvent::Key {
            key: nav_key_from_shortcut(shortcut),
            shift: chord && self.controls.modifiers.shift,
            ctrl: chord,
        });
        true
    }

    pub fn handle_text_input(&mut self, text: &str) -> bool {
        if self.screen == AppScreen::Chat {
            let now = self.now();
            if let Some(session) = self.session.as_mut() {
                session.chat.insert_text(text, now);
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
        session.game.set_paused(true);
        self.pause_return = None;
        self.set_screen(AppScreen::Pause);
        self.sound.stop_mining_loop(self.now());
    }

    pub(super) fn resume_game(&mut self) {
        let Some(session) = self.session.as_mut() else {
            self.set_screen(AppScreen::Title);
            return;
        };
        session.game.set_paused(false);
        match self.pause_return.take() {
            Some((screen, canvas)) => {
                self.set_screen(screen);
                self.client_canvas = canvas;
            }
            None => self.set_screen(AppScreen::Game),
        }
    }

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

    pub(super) fn save_and_quit_to_title(&mut self) {
        debug_assert!(
            self.session.as_ref().is_none_or(|s| !s.game.is_remote()),
            "save-and-quit is a HOST action; remote sessions disconnect"
        );
        self.end_session(AppScreen::Title);
    }

    pub(super) fn disconnect_to_title(&mut self) {
        debug_assert!(
            self.session.as_ref().is_some_and(|s| s.game.is_remote()),
            "Disconnect is a remote-session action"
        );
        self.end_session(AppScreen::Title);
    }

    pub(super) fn enter_connection_lost(&mut self, reason: String) {
        self.shell.set_disconnect_message(reason);
        self.end_session(AppScreen::ConnectionLost);
    }

    fn end_session(&mut self, next: AppScreen) {
        if let Some(session) = self.session.take() {
            self.retained_section_cache = Some(session.end());
        }
        self.teardown_game_scene();
        self.shell.refresh_worlds();
        self.set_screen(next);
    }

    pub(super) fn teardown_game_scene(&mut self) {
        self.sound.end_session(self.now());
        self.pause_return = None;
        self.client_overlays.clear();
        self.rebuild_action_table();
        self.renderer_world_clear_pending = true;
    }

    pub(super) fn play_selected_world(&mut self) {
        let Some(world) = self.shell.selected_world_info().cloned() else {
            return;
        };
        let seed = petramond::save::random_seed();
        self.open_world_checked(&world.dir_name, seed);
    }

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
        if petramond::save::read_world_settings(world_dir_name).auto_open_lan {
            self.open_lan();
        }
    }

    pub fn adopt_game(&mut self, game: crate::game::Game) {
        debug_assert!(
            self.session.is_none(),
            "a session is adopted only once the previous one has ended"
        );
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
        TextKey::F(n) => petramond_ui::NavKey::F(n),
    }
}

fn nav_key_from_shortcut(shortcut: TextShortcut) -> petramond_ui::NavKey {
    match shortcut {
        TextShortcut::SelectAll => petramond_ui::NavKey::SelectAll,
        TextShortcut::Cut => petramond_ui::NavKey::Cut,
        TextShortcut::Copy => petramond_ui::NavKey::Copy,
        TextShortcut::Paste => petramond_ui::NavKey::Paste,
        TextShortcut::Chord(ch) => petramond_ui::NavKey::Char(ch),
    }
}
