//! The player's inventory and menu actions: each is predicted locally where
//! the ledger allows and sent to the server, which owns the menu.

use super::Game;
use petramond::net::protocol::{ClientToServer, PlayerAction, ThrowAmount};

impl Game {
    /// Snapshot the predicted inventory and open a ledger entry for one
    /// predicted mutating action: `(can, id)`. When `can` is false the entry
    /// is track-only (no snapshot) and the caller must skip its local
    /// mutation — the ledger is at capacity until the server catches up.
    pub(super) fn begin_inventory_prediction(
        &mut self,
    ) -> (bool, petramond::net::protocol::ClientRequestId) {
        let can = self.prediction.can_predict();
        let snapshot = if can {
            crate::game::prediction::PredictionSnapshot::Inventory(self.self_view.inventory.clone())
        } else {
            crate::game::prediction::PredictionSnapshot::None
        };
        (can, self.prediction.begin(snapshot))
    }

    /// Like [`begin_inventory_prediction`](Self::begin_inventory_prediction),
    /// but the snapshot also captures the open menu mirror — for predictions
    /// that mutate a container-slot view alongside the cursor.
    pub(super) fn begin_menu_prediction(
        &mut self,
    ) -> (bool, petramond::net::protocol::ClientRequestId) {
        let can = self.prediction.can_predict();
        let snapshot = if can {
            crate::game::prediction::PredictionSnapshot::Menu {
                inventory: self.self_view.inventory.clone(),
                menu: self.menu_view.clone(),
            }
        } else {
            crate::game::prediction::PredictionSnapshot::None
        };
        (can, self.prediction.begin(snapshot))
    }

    /// Drop the player's held (active hotbar) item into the world via the in-game
    /// drop key. With `all`, the whole stack is thrown (Ctrl+Q); otherwise a
    /// single item (Q). No-op with an empty hand.
    pub fn drop_selected_item(&mut self, all: bool) {
        // P0 throw animation is client-owned: trigger when the hand holds
        // anything (the server never echoes the one-shot back).
        let slot = self.self_view.inventory.active_slot() as usize;
        self.local_hand_threw |= self.self_view.inventory.slot(slot).is_some();
        let (can, request_id) = self.begin_inventory_prediction();
        if can {
            let slot = self.self_view.inventory.active_slot() as usize;
            if all {
                let _ = self
                    .self_view
                    .inventory
                    .slot_mut(slot)
                    .and_then(|c| c.take());
            } else if let Some(cell) = self.self_view.inventory.slot_mut(slot) {
                if let Some(stack) = cell.as_mut() {
                    stack.count = stack.count.saturating_sub(1);
                    if stack.count == 0 {
                        *cell = None;
                    }
                }
            }
        }
        self.outbox.push(ClientToServer::Action(PlayerAction::Drop {
            all,
            request_id,
        }));
    }

    /// The F gesture in GAMEPLAY: swap the off-hand with the selected hotbar
    /// slot — one face of the unified hovered-slot swap
    /// ([`menu_swap_off_hand`](Self::menu_swap_off_hand)); the hotbar index
    /// is client-owned, so the client names the concrete slot.
    pub fn swap_off_hand(&mut self) {
        // The index is client-owned and `player.inventory` is its stated
        // owner (contents live on the replicated view; see the
        // client-prediction "what not to do" list).
        let active = self.player.inventory.active_slot() as usize;
        self.menu_swap_off_hand(petramond_world::gui_state::MenuSlot::Inventory(active));
    }

    /// Throw from the cursor-held stack out into the world (inventory drag-out
    /// then click outside the panel): the whole stack or a single item per
    /// `amount`. No-op when the cursor is empty.
    pub fn throw_cursor(&mut self, amount: ThrowAmount) {
        self.local_hand_threw |= self.self_view.inventory.cursor().is_some();
        let (can, request_id) = self.begin_inventory_prediction();
        if can {
            let cursor = self.self_view.inventory.cursor_mut();
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
        self.outbox
            .push(ClientToServer::Action(PlayerAction::ThrowCursor {
                amount,
                request_id,
            }));
    }

    /// Request one explicit craft by stable recipe key (`bulk` = shift-craft
    /// the maximum possible). The authoritative server revalidates station,
    /// ingredients, and output fit.
    pub fn craft_recipe(&mut self, recipe: &str, bulk: bool) {
        let request_id = self.prediction.begin_track_only();
        self.outbox.push(ClientToServer::CraftRecipe {
            recipe: recipe.to_owned(),
            bulk,
            request_id,
        });
    }

    /// The recipe browser's craftable-only filter preference (mirrored from
    /// the world's player data at join; client-owned afterwards).
    pub fn craft_craftable_only(&self) -> bool {
        self.player.craft_craftable_only
    }

    /// Flip the craftable-only filter locally and tell the server, which
    /// stores it on the player so it persists with the world's player data.
    pub fn set_craft_craftable_only(&mut self, craftable_only: bool) {
        if self.player.craft_craftable_only == craftable_only {
            return;
        }
        self.player.craft_craftable_only = craftable_only;
        self.outbox
            .push(ClientToServer::SetCraftFilter { craftable_only });
    }

    pub fn crafting_catalog(&self) -> &petramond_world::crafting::CraftingCatalog {
        &self.crafting
    }

    /// The local player's discovery record, mirrored from the server (the
    /// unlocked half — see `SelfRestore::unlocked_recipes`). The browser lists
    /// exactly these recipes.
    pub fn progression(&self) -> &petramond::player::Progression {
        &self.player.progression
    }

    #[cfg(test)]
    pub fn replica_for_test(&self) -> &petramond::world::World {
        &self.replica
    }

    /// Pin the locally-simulated player for tests that need a deterministic
    /// sampling location (the server session is placed by the caller).
    #[cfg(test)]
    pub fn place_player_for_test(&mut self, feet: petramond_math::world_pos::WorldPos) {
        self.player.pos = feet;
        self.player.vel = petramond_math::math::Vec3::ZERO;
    }

    pub fn replicated_inventory_revision(&self) -> u64 {
        self.self_view.inventory_revision
    }

    /// Install a browser catalog AND unlock all of it, the way a real session
    /// arrives (catalog from the handshake, unlocked set from the player's
    /// record). Tests about the browser are about presentation, not
    /// discovery; a test that wants a LOCKED recipe unlocks selectively.
    #[cfg(test)]
    pub fn set_crafting_catalog_for_test(
        &mut self,
        catalog: petramond_world::crafting::CraftingCatalog,
    ) {
        for recipe in catalog.iter() {
            self.player.progression.unlock(recipe.key());
        }
        self.crafting = catalog;
    }

    /// Whether the LOCAL cursor currently holds a stack, from the REPLICATED
    /// inventory (cursor rides `SelfState`). Gates the double-click gather,
    /// which only fires while a stack is being dragged; the gather verdict
    /// ships in the `MenuClick` message.
    pub fn cursor_has_stack(&self) -> bool {
        self.self_view.inventory.cursor().is_some()
    }

    /// Read-only state needed to build the UI snapshot for the LOCAL player's
    /// current menu — assembled from the client mirrors: replicated state plus
    /// any unresolved P1 prediction. No server-session reads.
    pub fn menu_read_model(&self) -> petramond::server::menu::MenuReadModel<'_> {
        let view = &self.menu_view;
        petramond::server::menu::MenuReadModel {
            inventory: &self.self_view.inventory,
            craft_output: view.craft_output,
            gui_state: view.gui_state.clone(),
            container: view.container.clone(),
        }
    }

    // Block-menu sessions open server-side before their `OpenScreen` event.
    // Inventory is the exception: the E key explicitly requests its session.

    pub fn request_open_inventory(&mut self) {
        self.outbox
            .push(ClientToServer::Action(PlayerAction::OpenInventory));
    }

    /// Ack of a server-opened GUI session — any kind, engine container or mod
    /// GUI (no-op; see above).
    pub fn open_gui_screen(
        &mut self,
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<petramond::menu::MenuAnchor>,
    ) {
        let _ = (kind, anchor);
    }

    /// Close the LOCAL player's open menu session. The server-side close
    /// (cursor/output stash, transient-input return, viewer release) runs ON THE TICK the
    /// message lands on; there is no client-side menu state to clear — the App
    /// owns which screen is up.
    pub fn close_open_menu(&mut self) {
        self.outbox
            .push(ClientToServer::Action(PlayerAction::CloseMenu));
    }
}
