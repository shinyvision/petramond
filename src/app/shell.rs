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
        if self.doc_ui_kind().is_none() {
            return false;
        }
        let key = match key {
            TextKey::Backspace => llama_ui::NavKey::Backspace,
            TextKey::Delete => llama_ui::NavKey::Delete,
            TextKey::Enter => llama_ui::NavKey::Enter,
            TextKey::Tab => llama_ui::NavKey::Tab,
            TextKey::ArrowLeft => llama_ui::NavKey::Left,
            TextKey::ArrowRight => llama_ui::NavKey::Right,
            TextKey::ArrowUp => llama_ui::NavKey::Up,
            TextKey::ArrowDown => llama_ui::NavKey::Down,
        };
        self.ui.push_input(llama_ui::InputEvent::Key {
            key,
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
        if self.doc_ui_kind().is_none() {
            return false;
        }
        let key = match shortcut {
            TextShortcut::SelectAll => llama_ui::NavKey::SelectAll,
            TextShortcut::Cut => llama_ui::NavKey::Cut,
            TextShortcut::Copy => llama_ui::NavKey::Copy,
            TextShortcut::Paste => llama_ui::NavKey::Paste,
        };
        self.ui
            .push_input(llama_ui::InputEvent::Key { key, shift: false });
        true
    }

    pub fn handle_text_input(&mut self, text: &str) -> bool {
        if self.doc_ui_kind().is_none() {
            return false;
        }
        for ch in text.chars() {
            self.ui.push_input(llama_ui::InputEvent::Char { ch });
        }
        true
    }

    pub fn take_quit_requested(&mut self) -> bool {
        std::mem::take(&mut self.quit_requested)
    }

    pub(super) fn open_pause(&mut self) {
        if self.game.is_none() {
            return;
        }
        self.screen = AppScreen::Pause;
        self.pointer.release_for_menu();
        self.audio.set_loop(None, now_seconds());
    }

    pub(super) fn resume_game(&mut self) {
        if self.game.is_none() {
            self.screen = AppScreen::Title;
            self.pointer.release_for_menu();
            return;
        }
        self.screen = AppScreen::Game;
        self.pointer.grab_for_gameplay();
    }

    pub(super) fn save_and_quit_to_title(&mut self) {
        if let Some(game) = self.game.as_mut() {
            game.save_all();
        }
        self.game = None;
        self.screen = AppScreen::Title;
        self.pointer.release_for_menu();
        self.audio.set_loop(None, now_seconds());
        self.scene.clear();
        self.hand = HandTriggers::default();
        self.sleep_interact_hand_t = 0.0;
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
        let rows = crate::assets::packs()
            .iter()
            .map(|p| ModPackRow {
                name: p.name.clone(),
                id: p.id.clone(),
                version: p.version.clone(),
                description: p.description.clone(),
                summary: p.summary.clone(),
            })
            .collect();
        self.world_settings = Some(WorldSettingsSession {
            dir_name: world.dir_name.clone(),
            world_name: world.name.clone(),
            rows,
            settings: crate::save::read_world_settings(&world.dir_name),
            selected: 0,
            renaming: false,
        });
        self.screen = AppScreen::WorldSettings;
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
        let Some(pack) = session.rows.get(row) else {
            return;
        };
        session.selected = row;
        let Some(id) = pack.id.clone() else {
            return; // content-only packs are always on
        };
        if !session.settings.disabled_mods.remove(&id) {
            session.settings.disabled_mods.insert(id);
        }
        if let Err(e) = crate::save::write_world_settings(&session.dir_name, &session.settings) {
            log::warn!(
                "could not write settings.json for world '{}': {e}",
                session.world_name
            );
        }
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
        self.game = Some(crate::game::Game::new(
            cam,
            world_dir_name,
            seed,
            self.render_dist,
        ));
        self.screen = AppScreen::Game;
        self.pointer.grab_for_gameplay();
        self.gui_router.reset_click_streak();
        self.hand = HandTriggers::default();
        self.sleep_interact_hand_t = 0.0;
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
