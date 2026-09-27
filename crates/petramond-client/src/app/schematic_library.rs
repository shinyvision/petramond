use super::session::Session;
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

#[derive(Default)]
pub(super) struct LibraryForm {
    pub page: LibraryPage,
    pub name: String,
    pub include_air: bool,
    pub pending_delete: Option<Entry>,
}

impl App {
    pub(super) fn open_requested_schematic_library(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.game.take_schematic_library_request() {
            return;
        }
        if self.screen.ui_open() {
            self.close_menu();
        }
        if self.screen == AppScreen::Game {
            self.set_screen(AppScreen::Schematics);
        }
    }

    pub(super) fn drive_schematics_screen(&mut self, screen: (u32, u32), now: f64) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.game.poll_schematic_library();
        if !session.game.schematic_choice_open() {
            self.close_screen();
            return;
        }
        self.ui.ensure_active(GuiKind::Schematics);
        self.drive_library_form("schematics_library_scroll", true);
        let Session {
            game,
            library_form: form,
            ..
        } = self.session.as_mut().expect("checked above");
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
        let selection = &game.tools.world.selection.selection;
        state.set("can_author", UiValue::Bool(!selection.is_empty()));
        state.set("notice", UiValue::Str(game.notice.clone()));
        self.ui.set_dynamic_images(thumbnails);
        self.ui
            .frame(GuiKind::Schematics, screen, now, Some([0.0, 0.0, 0.0, 0.6]));
        let mut leave = false;
        for event in self.ui.take_events() {
            let Session {
                game, library_form, ..
            } = self.session.as_mut().expect("open schematics screen");
            if library_form.handle(game, &event) {
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

    pub(super) fn drive_library_form(&mut self, scroll: &str, library_shown: bool) {
        let visible = self.visible_schematic_cards(scroll);
        let Some(Session {
            game,
            library_form: form,
            ..
        }) = self.session.as_mut()
        else {
            return;
        };
        if game.tools.library.take_saved() && form.page == LibraryPage::Save {
            form.page = LibraryPage::Library;
        }
        let cards_shown =
            library_shown && form.page == LibraryPage::Library && form.pending_delete.is_none();
        game.tools.library.request_thumbnails(match () {
            _ if !cards_shown => &[],
            _ if visible.is_empty() => &[0, 1, 2],
            _ => &visible,
        });
    }

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

pub(super) fn populate_library(
    game: &Game,
    form: &LibraryForm,
    state: &mut petramond_ui::UiState,
) -> Vec<petramond::modding::ClientImageData> {
    let tool = &game.tools.world.selection;
    let selection = &tool.selection;
    let entries = game.tools.library.entries();
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
                    if let Some(image) = game.tools.library.thumbnail(e) {
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
                self.pending_delete = game.tools.library.entries().get(i as usize).cloned();
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
