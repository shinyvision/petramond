//! The title-flow shell's own state: the world list and its selection, the
//! open per-screen page (World Settings / Create World sessions exist only
//! while their screen is up), the Connect to Server and account sessions, a
//! local world waiting on the Missing Mods screen, and the last disconnect
//! reason. Everything here is world-list I/O and form state — no
//! game session, no rendering — so shell screen controllers are handed this
//! directly instead of the whole `App`.

use petramond::save::settings::WorldSettings;
use petramond::save::WorldInfo;

use super::account::AccountSession;
use super::connect::ConnectSession;
use super::shell_docs::MissingWorld;

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
    pub(super) settings: WorldSettings,
    pub(super) selected: usize,
    /// The header's inline rename editor is open.
    pub(super) renaming: bool,
    pub(super) tab: SettingsTab,
    /// The world's seed (`level.dat` header); `None` before the first open.
    pub(super) seed: Option<u32>,
    /// Save-directory size, reported by the scan thread below.
    pub(super) size_bytes: Option<u64>,
    /// The off-thread size scan (region stores can hold many files); polled
    /// per frame, then dropped.
    pub(super) size_rx: Option<std::sync::mpsc::Receiver<u64>>,
}

/// The open Create World screen's state: the installed pack rows and the
/// settings the new world will be created with. Unlike World Settings there
/// is no world yet — mod toggles buffer here and `settings.json` is written
/// once on Create.
pub(super) struct CreateWorldSession {
    pub(super) rows: Vec<ModPackRow>,
    pub(super) settings: WorldSettings,
    pub(super) selected: usize,
    pub(super) tab: SettingsTab,
}

/// The per-screen state of the open shell page, owned by the page: opening a
/// page creates it, leaving the page drops it.
#[derive(Default)]
enum ShellPage {
    #[default]
    None,
    WorldSettings(WorldSettingsSession),
    CreateWorld(CreateWorldSession),
}

#[derive(Default)]
pub(super) struct ShellState {
    worlds: Vec<WorldInfo>,
    selected_world: Option<usize>,
    page: ShellPage,
    /// The Connect to Server session: entry fields, the off-thread connect
    /// worker's channel, and the mods a refused join reported missing.
    pub(super) connect: ConnectSession,
    /// The cached view of the stored Petramond sign-in plus the account worker
    /// (see [`super::account`]).
    pub(super) account: AccountSession,
    /// A local world waiting on the Missing Mods screen to be opened anyway.
    pub(super) missing_world: Option<MissingWorld>,
    /// Why the last session ended, shown by the Disconnected screen.
    disconnect_message: String,
}

/// One row per installed pack, in discovery order (parallel to
/// `petramond_world::assets::packs()` — the Mods-tab icon binding relies on that).
fn pack_rows() -> Vec<ModPackRow> {
    petramond_world::assets::packs()
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
    settings: &mut WorldSettings,
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

impl ShellState {
    pub(super) fn worlds(&self) -> &[WorldInfo] {
        &self.worlds
    }

    pub(super) fn selected_world(&self) -> Option<usize> {
        self.selected_world
    }

    pub(super) fn select_world(&mut self, index: Option<usize>) {
        self.selected_world = index;
    }

    /// The selected world, if the selection still names one.
    pub(super) fn selected_world_info(&self) -> Option<&WorldInfo> {
        self.selected_world.and_then(|index| self.worlds.get(index))
    }

    /// Clamp-move the world-list selection by `step` (the first row when
    /// nothing is selected).
    pub(super) fn move_world_selection(&mut self, step: i32) {
        if self.worlds.is_empty() {
            return;
        }
        let next = match self.selected_world {
            Some(i) => (i as i32 + step).clamp(0, self.worlds.len() as i32 - 1) as usize,
            None => 0,
        };
        self.selected_world = Some(next);
    }

    /// Re-read the world list from disk, dropping a selection past its end.
    pub(super) fn refresh_worlds(&mut self) {
        self.worlds = match petramond::save::list_worlds() {
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

    pub(super) fn disconnect_message(&self) -> &str {
        &self.disconnect_message
    }

    pub(super) fn set_disconnect_message(&mut self, reason: String) {
        self.disconnect_message = reason;
    }

    pub(super) fn world_settings(&self) -> Option<&WorldSettingsSession> {
        match &self.page {
            ShellPage::WorldSettings(session) => Some(session),
            _ => None,
        }
    }

    pub(super) fn world_settings_mut(&mut self) -> Option<&mut WorldSettingsSession> {
        match &mut self.page {
            ShellPage::WorldSettings(session) => Some(session),
            _ => None,
        }
    }

    pub(super) fn create_world(&self) -> Option<&CreateWorldSession> {
        match &self.page {
            ShellPage::CreateWorld(session) => Some(session),
            _ => None,
        }
    }

    pub(super) fn create_world_mut(&mut self) -> Option<&mut CreateWorldSession> {
        match &mut self.page {
            ShellPage::CreateWorld(session) => Some(session),
            _ => None,
        }
    }

    /// Leave the open page, dropping its state.
    pub(super) fn close_page(&mut self) {
        self.page = ShellPage::None;
    }

    /// Take the Create World session (closing its page) for the Create step.
    pub(super) fn take_create_world(&mut self) -> Option<CreateWorldSession> {
        match std::mem::take(&mut self.page) {
            ShellPage::CreateWorld(session) => Some(session),
            other => {
                self.page = other;
                None
            }
        }
    }

    /// Open the World Settings page for the selected world: the installed
    /// pack list (from pack discovery) plus the world's `settings.json`, and
    /// an off-thread scan of its save size. False when nothing is selected.
    pub(super) fn open_world_settings(&mut self) -> bool {
        let Some(world) = self.selected_world_info() else {
            return false;
        };
        let (size_tx, size_rx) = std::sync::mpsc::channel();
        let size_dir = world.dir_name.clone();
        std::thread::spawn(move || {
            let _ = size_tx.send(petramond::save::world_size_bytes(&size_dir));
        });
        self.page = ShellPage::WorldSettings(WorldSettingsSession {
            dir_name: world.dir_name.clone(),
            world_name: world.name.clone(),
            rows: pack_rows(),
            settings: petramond::save::read_world_settings(&world.dir_name),
            selected: 0,
            renaming: false,
            tab: SettingsTab::World,
            seed: petramond::save::read_world_seed(&world.dir_name),
            size_bytes: None,
            size_rx: Some(size_rx),
        });
        true
    }

    /// Adopt the off-thread save-dir size once it lands.
    pub(super) fn poll_world_size(&mut self) {
        if let Some(session) = self.world_settings_mut() {
            if let Some(rx) = &session.size_rx {
                if let Ok(bytes) = rx.try_recv() {
                    session.size_bytes = Some(bytes);
                    session.size_rx = None;
                }
            }
        }
    }

    /// Open the Create World page with a fresh session (all mods enabled).
    pub(super) fn open_create_world(&mut self) {
        self.page = ShellPage::CreateWorld(CreateWorldSession {
            rows: pack_rows(),
            settings: WorldSettings::default(),
            selected: 0,
            tab: SettingsTab::World,
        });
    }

    /// Flip one pack's enabled state for the open World Settings world and
    /// write `settings.json` immediately (a crash can't lose toggles; there
    /// is no unsaved state). Content-only packs (no id) are not toggleable.
    /// Takes effect the next time the world is OPENED — never live.
    pub(super) fn toggle_world_settings_row(&mut self, row: usize) {
        let Some(session) = self.world_settings_mut() else {
            return;
        };
        if session.rows.get(row).is_none() {
            return;
        }
        session.selected = row;
        if !toggle_pack_row(&session.rows, &mut session.settings, row) {
            return; // content-only packs are always on
        }
        self.write_world_settings();
    }

    /// Flip the keep-inventory-on-death world rule. Takes effect next open.
    pub(super) fn toggle_keep_inventory(&mut self) {
        if let Some(session) = self.world_settings_mut() {
            session.settings.keep_inventory = !session.settings.keep_inventory;
            self.write_world_settings();
        }
    }

    /// Flip the open-to-LAN-on-load world rule.
    pub(super) fn toggle_auto_open_lan(&mut self) {
        if let Some(session) = self.world_settings_mut() {
            session.settings.auto_open_lan = !session.settings.auto_open_lan;
            self.write_world_settings();
        }
    }

    /// Slide the world's day length (minutes). Live drags update the session
    /// (the label follows); only the committed release writes the file.
    pub(super) fn set_day_minutes(&mut self, minutes: u32, committed: bool) {
        if let Some(session) = self.world_settings_mut() {
            session.settings.day_minutes = minutes.clamp(10, 30);
            if committed {
                self.write_world_settings();
            }
        }
    }

    /// Write the open World Settings session's settings.json (the toggles'
    /// crash-can't-lose-it policy: every change writes immediately).
    fn write_world_settings(&self) {
        let Some(session) = self.world_settings() else {
            return;
        };
        if let Err(e) = petramond::save::write_world_settings(&session.dir_name, &session.settings)
        {
            log::warn!(
                "could not write settings.json for world '{}': {e}",
                session.world_name
            );
        }
    }

    /// Flip one pack's enabled state for the world being created. Buffered in
    /// the session only; written as the new world's `settings.json` on Create.
    pub(super) fn toggle_create_world_row(&mut self, row: usize) {
        let Some(session) = self.create_world_mut() else {
            return;
        };
        if session.rows.get(row).is_none() {
            return;
        }
        session.selected = row;
        toggle_pack_row(&session.rows, &mut session.settings, row);
    }

    /// Delete the selected world's save and its client-mod data, then clear
    /// the selection and re-read the list.
    pub(super) fn delete_selected_world(&mut self) {
        let Some(world) = self.selected_world_info().cloned() else {
            return;
        };
        if let Err(e) = petramond::save::delete_world(&world.dir_name) {
            log::warn!("could not delete world '{}': {e}", world.name);
        } else if let Err(e) =
            petramond::modding::client::delete_local_world_storage(&world.dir_name)
        {
            // Client-mod data (minimap exploration, waypoints) keys on the
            // save-directory name and lives outside the save — deleted with
            // the world, or a future world reusing the name inherits it.
            log::warn!(
                "could not delete client mod data for world '{}': {e}",
                world.name
            );
        }
        self.selected_world = None;
        self.refresh_worlds();
    }

    /// Replace the listed worlds without touching disk.
    #[cfg(test)]
    pub(super) fn set_worlds_for_test(&mut self, worlds: Vec<WorldInfo>) {
        self.worlds = worlds;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(name: &str) -> WorldInfo {
        WorldInfo {
            name: name.to_owned(),
            dir_name: name.to_owned(),
            has_level: true,
        }
    }

    #[test]
    fn selection_moves_within_the_list() {
        let mut shell = ShellState::default();
        shell.move_world_selection(1);
        assert_eq!(
            shell.selected_world(),
            None,
            "an empty list selects nothing"
        );
        shell.set_worlds_for_test(vec![world("a"), world("b")]);
        shell.move_world_selection(1);
        assert_eq!(
            shell.selected_world(),
            Some(0),
            "the first move selects the top"
        );
        shell.move_world_selection(5);
        assert_eq!(shell.selected_world(), Some(1), "clamped at the end");
        shell.move_world_selection(-5);
        assert_eq!(shell.selected_world(), Some(0), "clamped at the start");
        assert_eq!(
            shell.selected_world_info().map(|w| w.name.as_str()),
            Some("a")
        );
    }

    #[test]
    fn a_page_owns_its_session_and_leaving_drops_it() {
        let mut shell = ShellState::default();
        shell.open_create_world();
        assert!(shell.create_world().is_some());
        assert!(shell.world_settings().is_none(), "one page at a time");
        shell.toggle_create_world_row(usize::MAX);
        let session = shell.take_create_world().expect("the create page was open");
        assert_eq!(session.tab, SettingsTab::World);
        assert!(shell.create_world().is_none(), "taking closes the page");
        shell.open_create_world();
        shell.close_page();
        assert!(shell.create_world().is_none());
    }

    #[test]
    fn world_settings_need_a_selected_world() {
        let mut shell = ShellState::default();
        assert!(!shell.open_world_settings());
        assert!(shell.world_settings().is_none());
    }
}
