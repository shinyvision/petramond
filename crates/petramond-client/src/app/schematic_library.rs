//! The schematic library as a player uses it outside the creative menu: a
//! client-local screen a mod's choice opens, listing the personal library by
//! thumbnail, choosing one design, and saving the wand's selection. Its cards
//! and save page share their population with the creative menu's library.

use super::{App, AppScreen};
use crate::game::Game;
use petramond::schematic::library::Entry;
use petramond_ui::{UiEvent, UiValue};
use petramond_world::gui_state::GuiKind;
use std::collections::BTreeMap;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LibraryPage {
    #[default]
    Library,
    Save,
}

/// The form state every library screen shares: which page is up, the save
/// draft, and the card awaiting a delete confirmation.
#[derive(Default)]
pub(super) struct LibraryForm {
    pub page: LibraryPage,
    pub name: String,
    pub include_air: bool,
    /// Captured when the confirmation opens, so a refresh or sort cannot
    /// change what Delete removes.
    pub pending_delete: Option<Entry>,
}

impl App {
    /// Open the library for a choice the server just opened, over gameplay
    /// or over the menu whose button asked for it.
    pub(super) fn open_requested_schematic_library(&mut self) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        if !game.take_schematic_library_request() {
            return;
        }
        if self.screen.ui_open() {
            self.close_menu();
        }
        if !matches!(self.screen, AppScreen::Game) {
            return;
        }
        self.library_form.page = LibraryPage::Library;
        self.library_form.pending_delete = None;
        self.screen = AppScreen::Schematics;
        self.pointer.release_for_menu();
        self.gui_router.reset_click_streak();
    }

    pub(super) fn drive_schematics_screen(&mut self, screen: (u32, u32), now: f64) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        game.poll_schematic_library();
        if !game.schematic_choice_open() {
            self.close_screen();
            return;
        }
        self.ui.ensure_active(GuiKind::Schematics);
        self.drive_library_form("schematics_library_scroll", true);
        let game = self.game.as_mut().expect("checked above");
        let form = &self.library_form;
        let state = self.ui.state_mut();
        let thumbnails = populate_library(game, form, state);
        let browsing = form.pending_delete.is_none();
        state.set(
            "save_page",
            UiValue::Bool(browsing && form.page == LibraryPage::Save),
        );
        state.set(
            "library_page",
            UiValue::Bool(browsing && form.page == LibraryPage::Library),
        );
        let selection = &game.world_tools.selection.selection;
        state.set("can_author", UiValue::Bool(!selection.is_empty()));
        state.set("notice", UiValue::Str(game.notice.clone()));
        self.ui.set_dynamic_images(thumbnails);
        self.ui
            .frame(GuiKind::Schematics, screen, now, Some([0.0, 0.0, 0.0, 0.6]));
        let mut leave = false;
        for event in self.ui.take_events() {
            let game = self.game.as_mut().expect("open schematics screen");
            if self.library_form.handle(game, &event) {
                continue;
            }
            if let UiEvent::Click {
                id,
                item: Some(i),
                button: petramond_ui::PointerButton::Primary,
                ..
            } = event
            {
                if id == "use_schematic" {
                    leave |= game.choose_schematic(i as usize);
                }
            }
        }
        if leave {
            self.close_screen();
        }
    }

    /// The per-frame upkeep every library screen shares: a finished save
    /// returns from the Save page, and the cards in view ask for thumbnails.
    pub(super) fn drive_library_form(&mut self, scroll: &str, library_shown: bool) {
        let visible = self.visible_schematic_cards(scroll);
        let Some(game) = self.game.as_mut() else {
            return;
        };
        let form = &mut self.library_form;
        // Only from the Save page: a player who already left it stays put.
        if game.schematic_library.take_saved() && form.page == LibraryPage::Save {
            form.page = LibraryPage::Library;
        }
        let cards_shown =
            library_shown && form.page == LibraryPage::Library && form.pending_delete.is_none();
        game.schematic_library.request_thumbnails(match () {
            _ if !cards_shown => &[],
            // Nothing is laid out before the first frame.
            _ if visible.is_empty() => &[0, 1, 2],
            _ => &visible,
        });
    }

    /// Indices of the library cards the scroll view named `scroll` shows.
    pub(super) fn visible_schematic_cards(&self, scroll: &str) -> Vec<usize> {
        self.ui
            .out()
            .named
            .iter()
            .filter_map(|(key, rect)| {
                if key.id != "schematic_card" {
                    return None;
                }
                let clip = self.ui.out().rect(scroll)?;
                (rect.y < clip.y + clip.h && rect.y + rect.h > clip.y).then_some(key.item? as usize)
            })
            .collect()
    }
}

/// Publish the library cards, delete confirmation and save page shared by
/// every schematic library screen; returns the thumbnail images to bind.
pub(super) fn populate_library(
    game: &Game,
    form: &LibraryForm,
    state: &mut petramond_ui::UiState,
) -> Vec<petramond::modding::ClientImageData> {
    let tool = &game.world_tools.selection;
    let selection = &tool.selection;
    let entries = game.schematic_library.entries();
    state.set("browsing", UiValue::Bool(form.pending_delete.is_none()));
    state.set(
        "confirming_delete",
        UiValue::Bool(form.pending_delete.is_some()),
    );
    state.set(
        "delete_question",
        UiValue::Str(
            form.pending_delete
                .as_ref()
                .map(|e| format!("Delete \"{}\"?", e.metadata.name))
                .unwrap_or_default(),
        ),
    );
    state.set(
        "can_clear",
        UiValue::Bool(!selection.is_empty() || tool.has_pending_corner()),
    );
    state.set("schematic_name", UiValue::Str(form.name.clone()));
    state.set("include_air", UiValue::Bool(form.include_air));
    state.set(
        "selection_count",
        UiValue::Str(format!("{} cells selected", selection.len())),
    );
    state.set(
        "can_save",
        UiValue::Bool(!selection.is_empty() && !form.name.trim().is_empty()),
    );
    state.set("empty_library", UiValue::Bool(entries.is_empty()));
    let mut thumbnails = Vec::new();
    state.set(
        "schematics",
        UiValue::List(
            entries
                .iter()
                .map(|e| {
                    let image_key =
                        format!("schematic_{}_{:x}", e.path.display(), e.header.revision);
                    if let Some(image) = game.schematic_library.thumbnail(e) {
                        thumbnails.push(petramond::modding::ClientImageData {
                            key: image_key.clone(),
                            width: image.width as u16,
                            height: image.height as u16,
                            revision: u64::from(e.header.revision),
                            rgba: image.rgba.clone(),
                            recent_blits: Vec::new(),
                        });
                    }
                    BTreeMap::from([
                        ("name".into(), UiValue::Str(e.metadata.name.clone())),
                        ("thumbnail".into(), UiValue::Str(image_key)),
                        (
                            "details".into(),
                            UiValue::Str(format!(
                                "{} blocks · {}×{}×{}",
                                e.metadata.cell_count,
                                e.metadata.size[0],
                                e.metadata.size[1],
                                e.metadata.size[2]
                            )),
                        ),
                    ])
                })
                .collect::<Vec<_>>()
                .into(),
        ),
    );
    thumbnails
}

impl LibraryForm {
    /// The library events every library screen answers alike: the delete
    /// confirmation, the save page and deleting a card. `true` = handled.
    pub(super) fn handle(&mut self, game: &mut Game, event: &UiEvent) -> bool {
        let primary_click = match event {
            UiEvent::Click {
                id,
                item,
                button: petramond_ui::PointerButton::Primary,
                ..
            } => Some((id.as_str(), *item)),
            _ => None,
        };
        if self.pending_delete.is_some() {
            match primary_click {
                Some(("confirm_delete", _)) => {
                    if let Some(entry) = self.pending_delete.take() {
                        game.delete_schematic(entry);
                    }
                }
                Some(("cancel_delete", _)) => self.pending_delete = None,
                _ => {}
            }
            return true;
        }
        match (event, primary_click) {
            (UiEvent::TextChanged { id, text }, _) if id == "schematic_name" => {
                self.name = text.clone();
            }
            (UiEvent::Toggle { id, on, .. }, _) if id == "include_air" => self.include_air = *on,
            (_, Some(("delete_schematic", Some(i)))) => {
                self.pending_delete = game.schematic_library.entries().get(i as usize).cloned();
            }
            (_, Some(("save_schematic", _))) => game.save_selection(&self.name, self.include_air),
            (_, Some((id @ ("new_schematic" | "back_to_schematics"), _))) => {
                self.page = if id == "new_schematic" {
                    LibraryPage::Save
                } else {
                    LibraryPage::Library
                };
                game.notice.clear();
            }
            (_, Some(("clear_selection", _))) => game.clear_selection(),
            _ => return false,
        }
        true
    }
}
