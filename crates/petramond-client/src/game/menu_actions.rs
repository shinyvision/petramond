use super::Game;
use petramond::net::protocol::{ClientToServer, PlayerAction, ThrowAmount};
use petramond_world::gui_state::{ContainerView, GuiStateMap};
use petramond_world::inventory::Inventory;
use petramond_world::item::ItemStack;

pub struct MenuReadModel<'a> {
    pub inventory: &'a Inventory,
    pub craft_output: Option<ItemStack>,
    pub gui_state: Option<&'a std::sync::Arc<GuiStateMap>>,
    pub container: Option<&'a ContainerView>,
    pub anchor: Option<petramond::menu::MenuAnchor>,
}

impl Game {
    pub(super) fn begin_inventory_prediction(
        &mut self,
    ) -> (bool, petramond::net::protocol::ClientRequestId) {
        let can = self.prediction.can_predict();
        let snapshot = if can {
            crate::game::prediction::PredictionSnapshot::Inventory(
                self.replica.self_view.inventory.clone(),
            )
        } else {
            crate::game::prediction::PredictionSnapshot::None
        };
        (can, self.prediction.begin(snapshot))
    }

    pub(super) fn begin_menu_prediction(
        &mut self,
    ) -> (bool, petramond::net::protocol::ClientRequestId) {
        let can = self.prediction.can_predict();
        let snapshot = if can {
            crate::game::prediction::PredictionSnapshot::Menu {
                inventory: self.replica.self_view.inventory.clone(),
                menu: self.replica.menu_view.clone(),
            }
        } else {
            crate::game::prediction::PredictionSnapshot::None
        };
        (can, self.prediction.begin(snapshot))
    }

    pub fn drop_selected_item(&mut self, all: bool) {
        let slot = self.replica.self_view.inventory.active_slot() as usize;
        self.hand
            .latch_throw(self.replica.self_view.inventory.slot(slot).is_some());
        let (can, request_id) = self.begin_inventory_prediction();
        if can {
            let slot = self.replica.self_view.inventory.active_slot() as usize;
            if all {
                let _ = self
                    .replica
                    .self_view
                    .inventory
                    .slot_mut(slot)
                    .and_then(|c| c.take());
            } else if let Some(cell) = self.replica.self_view.inventory.slot_mut(slot) {
                if let Some(stack) = cell.as_mut() {
                    stack.count = stack.count.saturating_sub(1);
                    if stack.count == 0 {
                        *cell = None;
                    }
                }
            }
        }
        self.net.queue(ClientToServer::Action(PlayerAction::Drop {
            all,
            request_id,
        }));
    }

    pub fn swap_off_hand(&mut self) {
        let active = self.local.player.inventory.active_slot() as usize;
        self.menu_swap_off_hand(petramond_world::gui_state::MenuSlot::Inventory(active));
    }

    pub fn throw_cursor(&mut self, amount: ThrowAmount) {
        self.hand
            .latch_throw(self.replica.self_view.inventory.cursor().is_some());
        let (can, request_id) = self.begin_inventory_prediction();
        if can {
            let cursor = self.replica.self_view.inventory.cursor_mut();
            match amount {
                ThrowAmount::All => *cursor = None,
                ThrowAmount::One => {
                    if let Some(cur) = cursor.as_mut() {
                        cur.count = cur.count.saturating_sub(1);
                        if cur.count == 0 {
                            *cursor = None;
                        }
                    }
                }
            }
        }
        self.net
            .queue(ClientToServer::Action(PlayerAction::ThrowCursor {
                amount,
                request_id,
            }));
    }

    pub fn craft_recipe(&mut self, recipe: &str, bulk: bool) {
        let request_id = self.prediction.begin_track_only();
        self.net.queue(ClientToServer::CraftRecipe {
            recipe: recipe.to_owned(),
            bulk,
            request_id,
        });
    }

    pub fn craft_craftable_only(&self) -> bool {
        self.local.player.craft_craftable_only
    }

    pub fn set_craft_craftable_only(&mut self, craftable_only: bool) {
        if self.local.player.craft_craftable_only == craftable_only {
            return;
        }
        self.local.player.craft_craftable_only = craftable_only;
        self.net
            .queue(ClientToServer::SetCraftFilter { craftable_only });
    }

    pub fn crafting_catalog(&self) -> &petramond_world::crafting::CraftingCatalog {
        &self.replica.crafting
    }

    pub fn progression(&self) -> &petramond::player::Progression {
        &self.local.player.progression
    }

    #[cfg(test)]
    pub fn replica_for_test(&self) -> &petramond::world::ReplicaWorld {
        &self.replica.world
    }

    #[cfg(test)]
    pub fn place_player_for_test(&mut self, feet: petramond_math::world_pos::WorldPos) {
        self.local.player.pos = feet;
        self.local.player.vel = petramond_math::math::Vec3::ZERO;
    }

    pub fn replicated_inventory_revision(&self) -> u64 {
        self.replica.self_view.inventory_revision
    }

    #[cfg(test)]
    pub fn set_crafting_catalog_for_test(
        &mut self,
        catalog: petramond_world::crafting::CraftingCatalog,
    ) {
        for recipe in catalog.iter() {
            self.local.player.progression.unlock(recipe.key());
        }
        self.replica.crafting = catalog;
    }

    pub fn cursor_has_stack(&self) -> bool {
        self.replica.self_view.inventory.cursor().is_some()
    }

    pub fn menu_read_model(&self) -> MenuReadModel<'_> {
        let view = &self.replica.menu_view;
        MenuReadModel {
            inventory: &self.replica.self_view.inventory,
            craft_output: view.craft_output,
            gui_state: view.gui_state.as_ref(),
            container: view.container.as_ref(),
            anchor: self.replica.menu_anchor,
        }
    }

    pub fn request_open_inventory(&mut self) {
        self.replica.menu_anchor = None;
        self.net
            .queue(ClientToServer::Action(PlayerAction::OpenInventory));
    }

    pub fn open_gui_screen(
        &mut self,
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<petramond::menu::MenuAnchor>,
    ) {
        let _ = kind;
        self.replica.menu_anchor = anchor;
    }

    pub fn close_open_menu(&mut self) {
        self.replica.menu_anchor = None;
        self.net
            .queue(ClientToServer::Action(PlayerAction::CloseMenu));
    }
}
