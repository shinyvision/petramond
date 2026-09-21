//! The creative inventory: the item catalog and the schematic library behind
//! two icon tabs.

mod catalog;

use super::schematic_library::LibraryPage;
use super::App;
use petramond_ui::{UiEvent, UiValue};
use petramond_world::gui_state::GuiKind;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum CreativeTab {
    #[default]
    Items,
    Schematics,
}

/// The creative menu's own form state.
#[derive(Default)]
pub(super) struct CreativeMenu {
    pub tab: CreativeTab,
    pub query: String,
    catalog: catalog::Catalog,
}

impl App {
    pub(super) fn drive_creative_menu(&mut self, screen: (u32, u32), now: f64) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        game.poll_schematic_library();
        if game.take_paste_preview_ready() || !game.creative_mode() {
            self.close_screen();
            return;
        }
        self.ui.ensure_active(GuiKind::Creative);
        let on_library = self.creative_menu.tab == CreativeTab::Schematics;
        self.drive_library_form("creative_library_scroll", on_library);
        let game = self.game.as_mut().expect("checked above");
        let (items, item_rows) = self.creative_menu.catalog.view(&self.creative_menu.query);
        let hovered = self
            .ui
            .hover_item("creative_items")
            .and_then(|i| items.get(i));
        let state = self.ui.state_mut();
        let slot = game.menu_read_model().inventory.active_slot();
        self.hotbar_notice.populate(
            game.held_tool_setting().map(|label| (slot, label)),
            &mut game.notice,
            now,
            state,
        );
        let menu = &self.creative_menu;
        let page = on_library.then_some(self.library_form.page);
        state.set("tab", UiValue::I32(i32::from(on_library)));
        // Each key shows one page of the document; a delete confirmation
        // covers them all.
        for (key, shown) in [
            ("catalog_tab", None),
            ("selection_tab", Some(LibraryPage::Save)),
            ("library_tab", Some(LibraryPage::Library)),
        ] {
            let visible = self.library_form.pending_delete.is_none() && page == shown;
            state.set(key, UiValue::Bool(visible));
        }
        state.set("search", UiValue::Str(menu.query.clone()));
        state.set(
            "active_slot",
            UiValue::I32(i32::from(game.menu_read_model().inventory.active_slot())),
        );
        state.set("items", UiValue::List(item_rows));
        let thumbnails =
            super::schematic_library::populate_library(game, &self.library_form, state);
        let mut images = Vec::new();
        let hover_slot = self.ui.out().hover_slot.clone();
        let state = self.ui.state_mut();
        let drag = game.cursor_has_stack();
        state.set("cursor_has_stack", UiValue::Bool(drag));
        images.extend(if hovered.is_some() || drag {
            super::item_tooltip::populate_stack(
                if drag {
                    None
                } else {
                    hovered.map(|i| petramond_world::item::ItemStack::new(*i, 1))
                },
                state,
            )
        } else {
            super::item_tooltip::populate(game, hover_slot.as_ref(), state)
        });
        super::item_tooltip::populate_slot_tip(
            game,
            if drag || hovered.is_some() {
                None
            } else {
                hover_slot.as_ref()
            },
            state,
        );
        self.ui.set_extra_images(&images);
        self.ui.set_dynamic_images(thumbnails);
        self.ui
            .frame(GuiKind::Creative, screen, now, Some([0.0, 0.0, 0.0, 0.6]));
        for event in self.ui.take_events() {
            let game = self.game.as_mut().expect("open game menu");
            if self.library_form.handle(game, &event) {
                continue;
            }
            match event {
                UiEvent::TabSelect { id, index } if id == "creative_tabs" => {
                    self.creative_menu.tab = if index == 0 {
                        CreativeTab::Items
                    } else {
                        CreativeTab::Schematics
                    };
                    self.library_form.page = LibraryPage::Library;
                    game.notice.clear();
                }
                UiEvent::TextChanged { id, text } if id == "creative_search" => {
                    self.creative_menu.query = text
                }
                UiEvent::Click {
                    id,
                    item: Some(i),
                    button: petramond_ui::PointerButton::Primary,
                    ..
                } if id == "creative_item" => {
                    self.gui_router.reset_click_streak();
                    if game.cursor_has_stack() {
                        game.creative_discard_cursor();
                    } else if let Some(item) = items.get(i as usize) {
                        game.creative_pick(*item);
                    }
                }
                UiEvent::Click { id, .. } if id == "discard_creative_cursor" => {
                    self.gui_router.reset_click_streak();
                    game.creative_discard_cursor();
                }
                UiEvent::Click {
                    id,
                    item: Some(i),
                    button: petramond_ui::PointerButton::Primary,
                    ..
                } if id == "place_schematic" => game.begin_schematic_paste(i as usize),
                event => self.handle_inventory_event(GuiKind::Creative, event, now),
            }
        }
    }
}
