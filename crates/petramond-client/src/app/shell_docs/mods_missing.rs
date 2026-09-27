//! Missing Mods controller: the one "this needs content that is missing"
//! screen, for a join the server refused (its mod list) and for a local
//! world whose recorded world-affecting packs are not installed. Back (and
//! Enter, and ESC) returns to where the player came from: the connect screen
//! with the attempted address intact, or World Select. Only a local world
//! can be opened anyway. Get missing opens the content browser filtered to
//! the missing packs.

use super::{ScreenCtx, ShellCommand};
use crate::app::{App, AppScreen};
use petramond_ui::{NavKey, UiEvent, UiMap, UiState, UiValue};
use std::sync::Arc;

/// A local world held at the door because packs it recorded are missing.
pub(crate) struct MissingWorld {
    pub(crate) dir_name: String,
    pub(crate) seed: u32,
    pub(crate) missing: Vec<petramond::modding::modset::ModSetEntry>,
}

fn row(id: &str, version: &str) -> UiMap {
    let mut row = UiMap::new();
    row.insert("id".into(), UiValue::Str(id.to_owned()));
    row.insert("has_version".into(), UiValue::Bool(!version.is_empty()));
    let version = if version.is_empty() {
        String::new()
    } else {
        format!("v{version}")
    };
    row.insert("version".into(), UiValue::Str(version));
    row
}

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let (rows, intro): (Vec<UiMap>, &str) = match &ctx.shell.missing_world {
        Some(world) => (
            world
                .missing
                .iter()
                .map(|m| row(&m.id, &m.version))
                .collect(),
            "This world uses mods that are not installed:",
        ),
        None => (
            ctx.shell
                .connect
                .missing
                .iter()
                .map(|m| row(&m.id, &m.version))
                .collect(),
            "This server requires mods you don't have installed:",
        ),
    };
    state.set("missing_rows", UiValue::List(Arc::new(rows)));
    state.set("missing_intro", UiValue::Str(intro.to_owned()));
    state.set(
        "can_open_anyway",
        UiValue::Bool(ctx.shell.missing_world.is_some()),
    );
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } if id == "open_anyway" => {
            if let Some(world) = ctx.shell.missing_world.take() {
                ctx.request(ShellCommand::StartGame {
                    dir_name: world.dir_name,
                    seed: world.seed,
                });
            }
        }
        UiEvent::Click { id, .. } if id == "back" => ctx.request(ShellCommand::LeaveModsMissing),
        UiEvent::Click { id, .. } if id == "get_missing" => get_missing(ctx),
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => ctx.request(ShellCommand::LeaveModsMissing),
        _ => {}
    }
}

/// The browser, filtered to what is missing; its Back (and a relaunch's)
/// returns to where this screen's Back would.
fn get_missing(ctx: &mut ScreenCtx) {
    let (ids, back) = match ctx.shell.missing_world.take() {
        Some(world) => (
            world.missing.into_iter().map(|m| m.id).collect(),
            format!("world:{}", world.dir_name),
        ),
        None => (
            ctx.shell
                .connect
                .missing
                .iter()
                .map(|m| m.id.clone())
                .collect(),
            format!("connect:{}", ctx.shell.connect.addr),
        ),
    };
    ctx.request(ShellCommand::OpenContent {
        back: Some(back),
        filter: Some(ids),
    });
}

impl App {
    /// Open a local world, unless packs it recorded as changing what it holds
    /// are missing: then the Missing Mods screen asks first. A missing
    /// presentation-only pack never stops the open.
    pub(crate) fn open_world_checked(&mut self, dir_name: &str, seed: u32) {
        let settings = petramond::save::read_world_settings(dir_name);
        let missing: Vec<_> = petramond::modding::modset::missing(
            &petramond::save::world_dir(dir_name),
            &settings.disabled_mods,
        )
        .into_iter()
        .filter(|m| m.affects_world)
        .collect();
        if missing.is_empty() {
            return self.start_game(dir_name, seed);
        }
        for m in &missing {
            log::warn!("world '{dir_name}' uses '{}', which is not installed", m.id);
        }
        self.shell.missing_world = Some(MissingWorld {
            dir_name: dir_name.to_owned(),
            seed,
            missing,
        });
        self.set_screen(AppScreen::ModsMissing);
    }

    /// Back out of Missing Mods to where the player came from.
    pub(crate) fn leave_mods_missing(&mut self) {
        if self.shell.missing_world.take().is_some() {
            self.set_screen(AppScreen::WorldSelect);
        } else {
            self.reopen_connect_server();
        }
    }
}
