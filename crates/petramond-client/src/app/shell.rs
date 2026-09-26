//! App-level shell actions: entering and leaving game sessions (start,
//! adopt, pause/resume, LAN, save-and-quit, disconnect, connection loss) and
//! the text-input hooks that forward platform keyboard events into the
//! GUI-document runtime. The title flow's own state and world I/O live in
//! `super::shell_state`.

use super::{now_seconds, App, AppScreen};
use petramond_input::controls::{text_shortcut_from_key_code, TextKey, TextShortcut};
use petramond_render::camera::Camera;

impl App {
    /// Forward a text-editing key to the document UI. Returns whether it was
    /// consumed (false when no document screen is active).
    pub fn handle_text_key(&mut self, key: TextKey) -> bool {
        if self.screen == super::AppScreen::Chat {
            let now = now_seconds();
            match key {
                TextKey::Enter => {
                    if let Some(text) = self.chat.submit_or_close(now) {
                        if let Some(game) = self.game.as_mut() {
                            game.send_chat(text);
                        }
                    }
                    self.screen = super::AppScreen::Game;
                    self.controls.pointer.grab_for_gameplay();
                }
                _ => {
                    self.chat.edit_key(
                        nav_key_from_text_key(key),
                        self.controls.modifiers.shift,
                        self.controls.modifiers.ctrl,
                        None,
                        now,
                    );
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
        if self.screen == super::AppScreen::Chat {
            let key = nav_key_from_shortcut(shortcut);
            let now = now_seconds();
            let clipboard = self.ui.clipboard_mut();
            self.chat.edit_key(key, false, false, Some(clipboard), now);
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
        if self.screen == super::AppScreen::Chat {
            self.chat.insert_text(text, now_seconds());
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
        let Some(game) = self.game.as_mut() else {
            return;
        };
        // Pause is a protocol message: the server
        // thread keeps streaming/autosaving but skips the fixed ticks. The
        // screen switch below is what stops App::update calling Game::tick
        // (it still pumps the network — see update.rs).
        game.set_paused(true);
        self.screen = AppScreen::Pause;
        self.controls.pointer.release_for_menu();
        self.sound.stop_mining_loop(now_seconds());
    }

    pub(super) fn resume_game(&mut self) {
        // Pause-close cleanup: a stale LAN error must not greet the next open.
        self.session_ui.lan_error = None;
        let Some(game) = self.game.as_mut() else {
            self.screen = AppScreen::Title;
            self.controls.pointer.release_for_menu();
            return;
        };
        game.set_paused(false);
        self.screen = AppScreen::Game;
        self.controls.pointer.grab_for_gameplay();
    }

    /// The pause menu's Open to LAN: bind the default port into the running
    /// HOST server. Success shows the port label; failure shows inline.
    pub(super) fn open_lan(&mut self) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        let port = petramond::net::DEFAULT_PORT;
        match game.open_to_lan(port) {
            Ok(bound) => {
                self.session_ui.lan_port = Some(bound);
                self.session_ui.lan_error = None;
            }
            Err(e) => self.session_ui.lan_error = Some(format!("Couldn't open port {port}: {e}")),
        }
    }

    pub(super) fn save_and_quit_to_title(&mut self) {
        debug_assert!(
            self.game.as_ref().is_none_or(|g| !g.is_remote()),
            "save-and-quit is a HOST action; remote sessions disconnect"
        );
        if let Some(mut game) = self.game.take() {
            self.retained_section_cache = Some(game.take_section_cache());
            // Joins the server thread; it saves everything before exiting.
            game.shutdown();
        }
        self.screen = AppScreen::Title;
        self.teardown_game_scene();
    }

    /// Leave a REMOTE session (the pause menu's Disconnect): dropping the
    /// handle's sender makes the connection writer flush a farewell
    /// `Disconnect`; the server saves our player on its leave path. Nothing
    /// to save locally.
    pub(super) fn disconnect_to_title(&mut self) {
        debug_assert!(
            self.game.as_ref().is_some_and(|g| g.is_remote()),
            "Disconnect is a remote-session action"
        );
        if let Some(mut game) = self.game.take() {
            self.retained_section_cache = Some(game.take_section_cache());
            game.shutdown();
        }
        self.screen = AppScreen::Title;
        self.teardown_game_scene();
    }

    /// The involuntary exit: the server became unreachable (host thread
    /// crash, remote server close / connection loss). NO save — a crashed
    /// host has no server thread left to ask, and a remote server saves
    /// autonomously. Lands on the Disconnected screen with the reason.
    pub(super) fn enter_connection_lost(&mut self, reason: String) {
        if let Some(mut game) = self.game.take() {
            // The cache survives the session precisely for this path: a
            // reconnect's Join manifest claims it, skipping the re-stream.
            self.retained_section_cache = Some(game.take_section_cache());
            // For a crashed host thread this is a no-op join; for a remote
            // loss it drops the dead connection. Neither path saves.
            game.shutdown();
        }
        self.shell.set_disconnect_message(reason);
        self.screen = AppScreen::ConnectionLost;
        self.teardown_game_scene();
    }

    /// Shared post-session teardown (every quit/disconnect path): cursor,
    /// audio, scene, hand state, session UI (LAN status included), world-list
    /// refresh. The caller sets the target screen.
    fn teardown_game_scene(&mut self) {
        self.rebuild_action_table();
        self.controls.pointer.release_for_menu();
        // Mod-driven presentation state is session-scoped: the title screen
        // (or the next world) must never inherit this session's rain bed or
        // precipitation volumes.
        self.sound.end_session(now_seconds());
        self.presentation.ambient.clear();
        // Baked custom-shape item geometry is keyed by session-local block ids;
        // flush it so the next world's mods rebake instead of inheriting stale
        // shapes.
        petramond_world::block::item_shape_bake::clear();
        self.scene.clear();
        self.client_canvas = None;
        self.client_overlay_images.clear();
        self.hud_fx.reset_session();
        self.session_ui = Default::default();
        self.renderer_world_clear_pending = true;
        self.shell.refresh_worlds();
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
    pub fn adopt_game(&mut self, game: crate::game::Game) {
        self.game = Some(game);
        self.session_ui = Default::default();
        self.apply_particles();
        self.rebuild_action_table();
        self.screen = AppScreen::Game;
        self.controls.pointer.grab_for_gameplay();
        self.gui_router.reset_click_streak();
        self.hud_fx.reset_session();
        self.renderer_world_clear_pending = false;
        // A world saved while dead (quit from the death screen, or a crash)
        // reopens ON the death screen — a 0-health player must never resume
        // walking around.
        let dead = self
            .game
            .as_ref()
            .and_then(|g| g.player_health())
            .is_some_and(|h| h.current == 0);
        if dead {
            self.screen = AppScreen::Dead;
            self.controls.pointer.release_for_menu();
        }
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
