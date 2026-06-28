//! Game-side menu/session facade.
//!
//! `ContainerMenu` owns low-level slot behavior in `game/container`. This module
//! owns the `Game` boundary around that menu: opening edit targets, buffering menu
//! clicks for fixed ticks, close-session cleanup, and read models for the app UI.

use crate::controls::PointerButton;
use crate::crafting::CraftGrid;
use crate::gui::{ChestView, FurnaceView, MenuSlot, WorkbenchView};
use crate::inventory::Inventory;
use crate::mathh::IVec3;

#[cfg(test)]
use super::tick;
use super::Game;

/// Read-only game-side menu state consumed by the app's UI snapshot builder.
pub struct MenuReadModel<'a> {
    pub inventory: &'a Inventory,
    pub craft: &'a CraftGrid,
    pub furnace: Option<FurnaceView>,
    pub chest: Option<ChestView>,
    pub workbench: Option<WorkbenchView>,
}

impl Game {
    /// Whether the cursor currently holds a stack. Gates the double-click gather,
    /// which only fires while a stack is being dragged.
    pub fn cursor_has_stack(&self) -> bool {
        self.player.inventory.cursor().is_some()
    }

    /// Double-click gather: top up the cursor-held stack with every matching item
    /// in the inventory. See [`Inventory::collect_to_cursor`].
    #[cfg(test)]
    pub(crate) fn collect_to_cursor(&mut self) {
        self.player.inventory.collect_to_cursor();
    }

    /// Read-only state needed to build the UI snapshot for the current menu.
    pub fn menu_read_model(&self) -> MenuReadModel<'_> {
        MenuReadModel {
            inventory: &self.player.inventory,
            craft: self.menu.craft_grid(),
            furnace: self.menu.open_furnace_view(&self.world),
            chest: self.menu.open_chest_view(&self.world),
            workbench: self.menu.open_workbench_view(&self.recipes),
        }
    }

    /// Latch a hit-tested container click — resolved by the App to a [`MenuSlot`], a
    /// button, and Shift, with the App's double-click `gather` verdict — for the next game
    /// tick. Container edits mutate world state (chest / furnace contents) and the
    /// inventory, so they resolve on the tick like every other action — see
    /// [`tick_menu`](Self::tick_menu). The verdict is captured now, against the live
    /// cursor; since real clicks are more than a tick apart, each is applied before the
    /// next one is decided.
    pub fn menu_click(&mut self, slot: MenuSlot, button: PointerButton, shift: bool, gather: bool) {
        self.pending_menu_clicks.push((slot, button, shift, gather));
    }

    /// Apply the player actions latched this frame — container edits and item drops — at
    /// once, standing in for the game tick that resolves them in play. For App-level tests
    /// that drive the input routing and then assert the resulting inventory / world state
    /// (between two clicks a real tick interleaves, applying the first before the second is
    /// decided — call this there too).
    #[cfg(test)]
    pub(crate) fn apply_latched_actions_for_test(&mut self) {
        let mut events = tick::TickEvents::default();
        self.tick_drops(&mut events);
        self.tick_menu();
    }

    /// Apply this frame's latched container-menu clicks, in order, on the tick. Splits the
    /// disjoint `menu` / `world` / `inventory` borrows the menu needs and lends the
    /// recipes; the menu decodes each interaction keyed on its target.
    pub(super) fn tick_menu(&mut self) {
        for (slot, button, shift, gather) in std::mem::take(&mut self.pending_menu_clicks) {
            self.menu.click(
                &mut self.world,
                &mut self.player.inventory,
                &self.recipes,
                slot,
                button,
                shift,
                gather,
            );
        }
    }

    /// Configure the crafting grid for a screen of `cols×cols` (2 = inventory,
    /// 3 = table) and clear it. Called when a crafting screen opens.
    pub fn open_crafting(&mut self, cols: usize) {
        self.menu.open_crafting(cols, &self.recipes);
    }

    /// Begin a furnace-screen session at `pos` (the GUI's edit target).
    pub fn open_furnace_screen(&mut self, pos: IVec3) {
        self.menu.open_furnace_screen(&mut self.world, pos);
    }

    /// Begin a chest-screen session at `pos` (the GUI's edit target).
    pub fn open_chest_screen(&mut self, pos: IVec3) {
        self.menu.open_chest_screen(&mut self.world, pos);
    }

    /// Begin a furniture-workbench session (the input slot starts empty).
    pub fn open_workbench_screen(&mut self) {
        self.menu.open_workbench();
    }

    /// Close the open menu session in the app-required cleanup order:
    /// cursor stack, crafting grid, furnace, chest, then furniture workbench.
    pub fn close_open_menu(&mut self) {
        self.close_cursor_stack();
        self.close_crafting();
        self.close_furnace();
        self.close_chest();
        self.close_workbench();
    }

    /// End the furnace-screen session.
    fn close_furnace(&mut self) {
        self.menu.close_furnace();
    }

    /// Close the crafting grid: return every input item to the inventory (any
    /// overflow is thrown into the world), then clear the result. Overflow is
    /// gathered first, then queued after the menu call so the drop spawn path can
    /// borrow `world`/`cam` later on the fixed tick.
    fn close_crafting(&mut self) {
        let mut overflow = Vec::new();
        self.menu
            .close_crafting(&mut self.player.inventory, &self.recipes, |stack| {
                overflow.push(stack);
            });
        for stack in overflow {
            self.drop_queue.queue_stack(stack);
        }
    }

    /// End the chest-screen session.
    fn close_chest(&mut self) {
        self.menu.close_chest();
    }

    /// End the workbench session: return the input block to the inventory (overflow
    /// thrown into the world), like closing the crafting grid.
    fn close_workbench(&mut self) {
        let mut overflow = Vec::new();
        self.menu
            .close_workbench(&mut self.player.inventory, |stack| overflow.push(stack));
        for stack in overflow {
            self.drop_queue.queue_stack(stack);
        }
    }
}
