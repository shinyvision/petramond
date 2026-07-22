//! App-shell session state and actions shared by the document-backed shell
//! screen controllers (`super::shell_docs`): world list refresh, screen
//! transitions, world create/delete/settings I/O, and the text-input hooks
//! that forward platform keyboard events into the GUI-document runtime.

use super::{now_seconds, App, AppScreen, HandTriggers};
use crate::camera::Camera;
use crate::controls::{text_shortcut_from_key_code, TextKey, TextShortcut};
use crate::mathh::Vec3;

/// One World Settings row: an installed pack. Content-only packs (no `id`)
/// are listed but not toggleable — disable semantics are namespace-based and
/// they have none (their bare-key overrides are process-wide).
pub(super) struct ModPackRow {
    pub(super) name: String,
    pub(super) id: Option<String>,
    pub(super) version: Option<String>,
    pub(super) description: String,
    pub(super) summary: Option<String>,
}

/// Which tab of the tabbed World Settings / Create World screens is active.
/// Purely a shell UI concern; never persisted.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum SettingsTab {
    #[default]
    World,
    Mods,
}

impl SettingsTab {
    pub(super) fn index(self) -> i32 {
        match self {
            SettingsTab::World => 0,
            SettingsTab::Mods => 1,
        }
    }

    pub(super) fn from_index(index: u32) -> SettingsTab {
        if index == 1 {
            SettingsTab::Mods
        } else {
            SettingsTab::World
        }
    }
}

/// The open World Settings screen's state: which world, the installed pack
/// rows, and the world's disabled set (mirrors `settings.json`; every toggle
/// writes the file immediately).
pub(super) struct WorldSettingsSession {
    pub(super) dir_name: String,
    pub(super) world_name: String,
    pub(super) rows: Vec<ModPackRow>,
    pub(super) settings: crate::save::settings::WorldSettings,
    pub(super) selected: usize,
    /// The header's inline rename editor is open.
    pub(super) renaming: bool,
    pub(super) tab: SettingsTab,
    /// The world's seed (`level.dat` header); `None` before the first open.
    pub(super) seed: Option<u32>,
    /// Save-directory size, reported by the scan thread below.
    pub(super) size_bytes: Option<u64>,
    /// The off-thread size scan (region stores can hold many files); the
    /// controller's prepare polls it, then drops it.
    pub(super) size_rx: Option<std::sync::mpsc::Receiver<u64>>,
}

/// The open Create World screen's state: the installed pack rows and the
/// settings the new world will be created with. Unlike World Settings there
/// is no world yet — mod toggles buffer here and `settings.json` is written
/// once on Create.
pub(super) struct CreateWorldSession {
    pub(super) rows: Vec<ModPackRow>,
    pub(super) settings: crate::save::settings::WorldSettings,
    pub(super) selected: usize,
    pub(super) tab: SettingsTab,
}

/// One row per installed pack, in discovery order (parallel to
/// `crate::assets::packs()` — the Mods-tab icon binding relies on that).
fn pack_rows() -> Vec<ModPackRow> {
    crate::assets::packs()
        .iter()
        .map(|p| ModPackRow {
            name: p.name.clone(),
            id: p.id.clone(),
            version: p.version.clone(),
            description: p.description.clone(),
            summary: p.summary.clone(),
        })
        .collect()
}

/// Flip one pack row's enabled state in `settings`. Returns false for
/// content-only packs (no id — always on) and out-of-range rows.
pub(super) fn toggle_pack_row(
    rows: &[ModPackRow],
    settings: &mut crate::save::settings::WorldSettings,
    row: usize,
) -> bool {
    let Some(Some(id)) = rows.get(row).map(|pack| pack.id.clone()) else {
        return false;
    };
    if !settings.disabled_mods.remove(&id) {
        settings.disabled_mods.insert(id);
    }
    true
}

impl App {
    pub(super) fn refresh_worlds(&mut self) {
        self.worlds = match crate::save::list_worlds() {
            Ok(worlds) => worlds,
            Err(e) => {
                log::warn!("could not list worlds: {e}");
                Vec::new()
            }
        };
        if let Some(selected) = self.selected_world {
            if selected >= self.worlds.len() {
                self.selected_world = None;
            }
        }
    }

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
                    self.pointer.grab_for_gameplay();
                }
                _ => {
                    self.chat
                        .edit_key(nav_key_from_text_key(key), self.modifiers.shift, None, now);
                }
            }
            return true;
        }
        if self.doc_ui_kind().is_none() {
            return false;
        }
        self.ui.push_input(petramond_ui::InputEvent::Key {
            key: nav_key_from_text_key(key),
            shift: self.modifiers.shift,
        });
        true
    }

    /// Resolve a physical key + tracked modifiers into a text shortcut and
    /// forward it. Clipboard access lives inside the document UI (`AppUi`
    /// owns its own clipboard), so no host clipboard is threaded through.
    pub fn handle_text_shortcut_code(&mut self, code: winit::keyboard::KeyCode) -> bool {
        let Some(shortcut) = text_shortcut_from_key_code(code, self.modifiers) else {
            return false;
        };
        self.handle_text_shortcut(shortcut)
    }

    pub fn handle_text_shortcut(&mut self, shortcut: TextShortcut) -> bool {
        if self.screen == super::AppScreen::Chat {
            let key = nav_key_from_shortcut(shortcut);
            let now = now_seconds();
            let clipboard = self.ui.clipboard_mut();
            self.chat.edit_key(key, false, Some(clipboard), now);
            return true;
        }
        if self.doc_ui_kind().is_none() {
            return false;
        }
        self.ui.push_input(petramond_ui::InputEvent::Key {
            key: nav_key_from_shortcut(shortcut),
            shift: false,
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
        self.pointer.release_for_menu();
        self.audio.set_loop(None, now_seconds());
    }

    pub(super) fn resume_game(&mut self) {
        // Pause-close cleanup: a stale LAN error must not greet the next open.
        self.lan_error = None;
        let Some(game) = self.game.as_mut() else {
            self.screen = AppScreen::Title;
            self.pointer.release_for_menu();
            return;
        };
        game.set_paused(false);
        self.screen = AppScreen::Game;
        self.pointer.grab_for_gameplay();
    }

    /// The pause menu's Open to LAN: bind the default port into the running
    /// HOST server. Success shows the port label; failure shows inline.
    pub(super) fn open_lan(&mut self) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        let port = crate::net::DEFAULT_PORT;
        match game.open_to_lan(port) {
            Ok(bound) => {
                self.lan_port = Some(bound);
                self.lan_error = None;
            }
            Err(e) => self.lan_error = Some(format!("Couldn't open port {port}: {e}")),
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
        self.disconnect_message = reason;
        self.screen = AppScreen::ConnectionLost;
        self.teardown_game_scene();
    }

    /// Shared post-session teardown (every quit/disconnect path): cursor,
    /// audio, scene, hand state, LAN bookkeeping, world-list refresh. The
    /// caller sets the target screen.
    fn teardown_game_scene(&mut self) {
        self.rebuild_action_table();
        self.pointer.release_for_menu();
        self.audio.set_loop(None, now_seconds());
        // Mod-driven presentation state is session-scoped: the title screen
        // (or the next world) must never inherit this session's rain bed or
        // precipitation volumes.
        self.audio.stop_mod_loops();
        self.presentation.ambient.clear();
        // Baked custom-shape item geometry is keyed by session-local block ids;
        // flush it so the next world's mods rebake instead of inheriting stale
        // shapes.
        crate::render::item_shape_bake::clear();
        self.scene.clear();
        self.client_canvas = None;
        self.client_overlay_images.clear();
        self.hand = HandTriggers::default();
        self.sleep_interact_hand_t = 0.0;
        self.lan_port = None;
        self.lan_error = None;
        self.renderer_world_clear_pending = true;
        self.refresh_worlds();
    }

    pub(super) fn play_selected_world(&mut self) {
        let Some(index) = self.selected_world else {
            return;
        };
        let Some(world) = self.worlds.get(index).cloned() else {
            return;
        };
        let seed = crate::save::random_seed();
        self.start_game(&world.dir_name, seed);
    }

    pub(super) fn open_delete_world_confirm(&mut self) {
        if self
            .selected_world
            .and_then(|index| self.worlds.get(index))
            .is_none()
        {
            return;
        }
        self.screen = AppScreen::DeleteWorld;
        self.pointer.release_for_menu();
    }

    /// Open the World Settings screen for the selected world: the installed
    /// pack list (from pack discovery) plus the world's `settings.json`.
    pub(super) fn open_world_settings(&mut self) {
        let Some(world) = self.selected_world.and_then(|index| self.worlds.get(index)) else {
            return;
        };
        let (size_tx, size_rx) = std::sync::mpsc::channel();
        let size_dir = world.dir_name.clone();
        std::thread::spawn(move || {
            let _ = size_tx.send(crate::save::world_size_bytes(&size_dir));
        });
        self.world_settings = Some(WorldSettingsSession {
            dir_name: world.dir_name.clone(),
            world_name: world.name.clone(),
            rows: pack_rows(),
            settings: crate::save::read_world_settings(&world.dir_name),
            selected: 0,
            renaming: false,
            tab: SettingsTab::World,
            seed: crate::save::read_world_seed(&world.dir_name),
            size_bytes: None,
            size_rx: Some(size_rx),
        });
        self.screen = AppScreen::WorldSettings;
        self.pointer.release_for_menu();
    }

    /// Open the Create World screen with a fresh session (all mods enabled).
    pub(super) fn open_create_world(&mut self) {
        self.create_world = Some(CreateWorldSession {
            rows: pack_rows(),
            settings: crate::save::settings::WorldSettings::default(),
            selected: 0,
            tab: SettingsTab::World,
        });
        self.screen = AppScreen::CreateWorld;
        self.pointer.release_for_menu();
    }

    /// Flip one pack's enabled state for the open World Settings world and
    /// write `settings.json` immediately (a crash can't lose toggles; there
    /// is no unsaved state). Content-only packs (no id) are not toggleable.
    /// Takes effect the next time the world is OPENED — never live.
    pub(super) fn toggle_world_settings_row(&mut self, row: usize) {
        let Some(session) = self.world_settings.as_mut() else {
            return;
        };
        if session.rows.get(row).is_none() {
            return;
        }
        session.selected = row;
        if !toggle_pack_row(&session.rows, &mut session.settings, row) {
            return; // content-only packs are always on
        }
        self.write_world_settings_session();
    }

    /// Write the open World Settings session's settings.json (the toggles'
    /// crash-can't-lose-it policy: every change writes immediately).
    fn write_world_settings_session(&mut self) {
        let Some(session) = self.world_settings.as_ref() else {
            return;
        };
        if let Err(e) = crate::save::write_world_settings(&session.dir_name, &session.settings) {
            log::warn!(
                "could not write settings.json for world '{}': {e}",
                session.world_name
            );
        }
    }

    /// Flip the keep-inventory-on-death world rule. Takes effect next open.
    pub(super) fn toggle_keep_inventory(&mut self) {
        if let Some(session) = self.world_settings.as_mut() {
            session.settings.keep_inventory = !session.settings.keep_inventory;
            self.write_world_settings_session();
        }
    }

    /// Flip the open-to-LAN-on-load world rule.
    pub(super) fn toggle_auto_open_lan(&mut self) {
        if let Some(session) = self.world_settings.as_mut() {
            session.settings.auto_open_lan = !session.settings.auto_open_lan;
            self.write_world_settings_session();
        }
    }

    /// Slide the world's day length (minutes). Live drags update the session
    /// (the label follows); only the committed release writes the file.
    pub(super) fn set_day_minutes(&mut self, minutes: u32, committed: bool) {
        if let Some(session) = self.world_settings.as_mut() {
            session.settings.day_minutes = minutes.clamp(10, 30);
            if committed {
                self.write_world_settings_session();
            }
        }
    }

    /// Flip one pack's enabled state for the world being created. Buffered in
    /// the session only; written as the new world's `settings.json` on Create.
    pub(super) fn toggle_create_world_row(&mut self, row: usize) {
        let Some(session) = self.create_world.as_mut() else {
            return;
        };
        if session.rows.get(row).is_none() {
            return;
        }
        session.selected = row;
        toggle_pack_row(&session.rows, &mut session.settings, row);
    }

    pub(super) fn delete_selected_world(&mut self) {
        let Some(world) = self
            .selected_world
            .and_then(|index| self.worlds.get(index))
            .cloned()
        else {
            self.screen = AppScreen::WorldSelect;
            self.pointer.release_for_menu();
            return;
        };
        if let Err(e) = crate::save::delete_world(&world.dir_name) {
            log::warn!("could not delete world '{}': {e}", world.name);
        } else if let Err(e) = crate::modding::client::delete_local_world_storage(&world.dir_name) {
            // Client-mod data (minimap exploration, waypoints) keys on the
            // save-directory name and lives outside the save — deleted with
            // the world, or a future world reusing the name inherits it.
            log::warn!(
                "could not delete client mod data for world '{}': {e}",
                world.name
            );
        }
        self.selected_world = None;
        self.screen = AppScreen::WorldSelect;
        self.pointer.release_for_menu();
        self.refresh_worlds();
    }

    /// Open (or create) the world saved under `world_dir_name` —
    /// `WorldInfo::dir_name`, NOT the display name: renames change only the
    /// display name, so opening by name would silently start a fresh world.
    pub(crate) fn start_game(&mut self, world_dir_name: &str, seed: u32) {
        let cam = Camera::new(
            Vec3::new(8.0, 90.0, 8.0),
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
        if crate::save::read_world_settings(world_dir_name).auto_open_lan {
            self.open_lan();
        }
    }

    /// Install a freshly-built game session and enter gameplay — the shared
    /// tail of [`start_game`] (which builds the session, spawning the server
    /// thread) and the test fixtures (which build a loopback-piped session).
    pub(crate) fn adopt_game(&mut self, game: crate::game::Game) {
        self.game = Some(game);
        self.apply_particles();
        self.rebuild_action_table();
        self.screen = AppScreen::Game;
        self.pointer.grab_for_gameplay();
        self.gui_router.reset_click_streak();
        self.hand = HandTriggers::default();
        self.sleep_interact_hand_t = 0.0;
        self.lan_port = None;
        self.lan_error = None;
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
            self.pointer.release_for_menu();
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
