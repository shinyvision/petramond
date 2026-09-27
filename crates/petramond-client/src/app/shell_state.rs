use petramond::save::settings::WorldSettings;
use petramond::save::WorldInfo;

use super::account::AccountSession;
use super::connect::ConnectSession;
use super::shell_docs::MissingWorld;

pub(super) struct ModPackRow {
    pub(super) name: String,
    pub(super) id: Option<String>,
    pub(super) version: Option<String>,
    pub(super) description: String,
    pub(super) summary: Option<String>,
}

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

pub(super) struct WorldSettingsSession {
    pub(super) dir_name: String,
    pub(super) world_name: String,
    pub(super) rows: Vec<ModPackRow>,
    pub(super) settings: WorldSettings,
    pub(super) selected: usize,
    pub(super) renaming: bool,
    pub(super) tab: SettingsTab,
    pub(super) seed: Option<u32>,
    pub(super) size_bytes: Option<u64>,
    pub(super) size_rx: Option<std::sync::mpsc::Receiver<u64>>,
}

pub(super) struct CreateWorldSession {
    pub(super) rows: Vec<ModPackRow>,
    pub(super) settings: WorldSettings,
    pub(super) selected: usize,
    pub(super) tab: SettingsTab,
}

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
    pub(super) connect: ConnectSession,
    pub(super) account: AccountSession,
    pub(super) missing_world: Option<MissingWorld>,
    disconnect_message: String,
}

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

    pub(super) fn selected_world_info(&self) -> Option<&WorldInfo> {
        self.selected_world.and_then(|index| self.worlds.get(index))
    }

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

    pub(super) fn close_page(&mut self) {
        self.page = ShellPage::None;
    }

    pub(super) fn take_create_world(&mut self) -> Option<CreateWorldSession> {
        match std::mem::take(&mut self.page) {
            ShellPage::CreateWorld(session) => Some(session),
            other => {
                self.page = other;
                None
            }
        }
    }

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

    pub(super) fn open_create_world(&mut self) {
        self.page = ShellPage::CreateWorld(CreateWorldSession {
            rows: pack_rows(),
            settings: WorldSettings::default(),
            selected: 0,
            tab: SettingsTab::World,
        });
    }

    pub(super) fn toggle_world_settings_row(&mut self, row: usize) {
        let Some(session) = self.world_settings_mut() else {
            return;
        };
        if session.rows.get(row).is_none() {
            return;
        }
        session.selected = row;
        if !toggle_pack_row(&session.rows, &mut session.settings, row) {
            return;
        }
        self.write_world_settings();
    }

    pub(super) fn toggle_keep_inventory(&mut self) {
        if let Some(session) = self.world_settings_mut() {
            session.settings.keep_inventory = !session.settings.keep_inventory;
            self.write_world_settings();
        }
    }

    pub(super) fn toggle_auto_open_lan(&mut self) {
        if let Some(session) = self.world_settings_mut() {
            session.settings.auto_open_lan = !session.settings.auto_open_lan;
            self.write_world_settings();
        }
    }

    pub(super) fn set_day_minutes(&mut self, minutes: u32, committed: bool) {
        if let Some(session) = self.world_settings_mut() {
            session.settings.day_minutes = minutes.clamp(10, 30);
            if committed {
                self.write_world_settings();
            }
        }
    }

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

    pub(super) fn delete_selected_world(&mut self) {
        let Some(world) = self.selected_world_info().cloned() else {
            return;
        };
        if let Err(e) = petramond::save::delete_world(&world.dir_name) {
            log::warn!("could not delete world '{}': {e}", world.name);
        } else if let Err(e) =
            petramond::modding::client::delete_local_world_storage(&world.dir_name)
        {
            log::warn!(
                "could not delete client mod data for world '{}': {e}",
                world.name
            );
        }
        self.selected_world = None;
        self.refresh_worlds();
    }

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
