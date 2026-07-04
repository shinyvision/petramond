//! Game-side menu/session facade.
//!
//! `ContainerMenu` owns low-level slot behavior in `game/container`. This module
//! owns the `Game` boundary around that menu: opening edit targets, buffering menu
//! clicks for fixed ticks, close-session cleanup, and read models for the app UI.

use crate::controls::PointerButton;
use crate::crafting::CraftGrid;
use crate::events::{ContainerKind, PostEvent, SimCtx};
use crate::gui::{ChestView, FurnaceView, GuiStateMap, MenuSlot, WorkbenchView};
use crate::inventory::Inventory;
use crate::mathh::IVec3;

use super::container::ContainerTarget;
use super::tick::TickEvents;
use super::Game;

/// Read-only game-side menu state consumed by the app's UI snapshot builder.
pub struct MenuReadModel<'a> {
    pub inventory: &'a Inventory,
    pub craft: &'a CraftGrid,
    pub furnace: Option<FurnaceView>,
    pub chest: Option<ChestView>,
    pub workbench: Option<WorkbenchView>,
    /// The open mod GUI's state map (a shared snapshot), or `None` when the
    /// open session is not a mod GUI.
    pub gui_state: Option<std::sync::Arc<GuiStateMap>>,
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
            gui_state: matches!(self.menu.target(), ContainerTarget::ModGui { .. })
                .then(|| self.world.gui_state_snapshot()),
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
        let mut events = TickEvents::default();
        self.tick_drops(&mut events);
        self.tick_menu(&mut events);
    }

    /// Apply this frame's latched container-menu clicks, in order, on the tick. Splits the
    /// disjoint `menu` / `world` / `inventory` borrows the menu needs and lends the
    /// recipes; the menu decodes each interaction keyed on its target. Widget
    /// (button) clicks mutate no container — they dispatch to the open mod
    /// GUI's owning mod instead.
    pub(super) fn tick_menu(&mut self, events: &mut TickEvents) {
        for (slot, button, shift, gather) in std::mem::take(&mut self.pending_menu_clicks) {
            if let MenuSlot::Widget(id) = slot {
                // Only a primary click activates a button (a right-click over
                // one is consumed by the panel but triggers nothing).
                if button == PointerButton::Primary {
                    self.dispatch_gui_click(id, events);
                }
                continue;
            }
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

    /// Dispatch a latched button click to the open mod GUI's OWNING mod (the
    /// pack whose namespace the kind key carries) as a `gui_click` GuestCall,
    /// on the tick. Engine kinds have no owner (no engine buttons exist) and
    /// a click with no mod GUI session open dispatches nothing.
    fn dispatch_gui_click(&mut self, widget_id: crate::gui::WidgetId, events: &mut TickEvents) {
        let ContainerTarget::ModGui { kind, pos } = self.menu.target() else {
            return;
        };
        let Some(kind_key) = crate::gui::kind_key(kind) else {
            return;
        };
        let mut ctx = SimCtx {
            world: &mut self.world,
            player: &mut self.player,
            feed: events,
            queue: self.bus.queue_mut(),
        };
        self.mods
            .dispatch_gui_click(&mut ctx, kind_key, widget_id, pos.map(|p| [p.x, p.y, p.z]));
    }

    /// Configure the crafting grid for a screen of `cols×cols` (2 = inventory,
    /// 3 = table) and clear it. Called when a crafting screen opens.
    pub fn open_crafting(&mut self, cols: usize) {
        self.menu.open_crafting(cols, &self.recipes);
        self.emit_container_opened();
    }

    /// Begin a furnace-screen session at `pos` (the GUI's edit target).
    pub fn open_furnace_screen(&mut self, pos: IVec3) {
        self.menu.open_furnace_screen(&mut self.world, pos);
        self.emit_container_opened();
    }

    /// Begin a chest-screen session at `pos` (the GUI's edit target).
    pub fn open_chest_screen(&mut self, pos: IVec3) {
        self.menu.open_chest_screen(&mut self.world, pos);
        self.emit_container_opened();
    }

    /// Begin a furniture-workbench session (the input slot starts empty).
    pub fn open_workbench_screen(&mut self) {
        self.menu.open_workbench();
        self.emit_container_opened();
    }

    /// Begin a mod GUI session for `kind`, opened from block `pos` (`None`
    /// for a programmatic `GuiOpen`). The session's state map starts empty —
    /// cleared here so no session can read a predecessor's values.
    pub fn open_mod_gui_screen(&mut self, kind: crate::gui::GuiKind, pos: Option<IVec3>) {
        self.world.gui_state_clear();
        self.menu.open_mod_gui(kind, pos);
        self.emit_container_opened();
    }

    /// Close the open menu session in the app-required cleanup order:
    /// cursor stack, crafting grid, furnace, chest, furniture workbench, then
    /// any mod GUI (whose session state map is cleared with it).
    pub fn close_open_menu(&mut self) {
        // `container_closed` for whatever session was actually open. Emitted (not
        // dispatched) here: the app calls this per-frame, so the handler runs at
        // the next tick's first drain point, like every per-frame-queued event.
        if let Some((kind, pos)) = container_event_key(self.menu.target()) {
            self.bus.emit(PostEvent::ContainerClosed { kind, pos });
        }
        self.close_cursor_stack();
        self.close_crafting();
        self.close_furnace();
        self.close_chest();
        self.close_workbench();
        self.close_mod_gui();
    }

    /// `container_opened` for the session that just began. The `Game::open_*`
    /// methods are the single funnel every container screen opens through (whether
    /// from a block interact or the inventory key), so the event fires exactly once
    /// per session.
    fn emit_container_opened(&mut self) {
        if let Some((kind, pos)) = container_event_key(self.menu.target()) {
            self.bus.emit(PostEvent::ContainerOpened { kind, pos });
        }
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

    /// End the mod GUI session and clear its state map.
    fn close_mod_gui(&mut self) {
        if matches!(self.menu.target(), ContainerTarget::ModGui { .. }) {
            self.world.gui_state_clear();
            self.menu.close_mod_gui();
        }
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

/// The `container_opened`/`container_closed` payload for a menu target, or `None`
/// when no container session is involved.
fn container_event_key(target: ContainerTarget) -> Option<(ContainerKind, Option<IVec3>)> {
    match target {
        ContainerTarget::None => None,
        ContainerTarget::Inventory => Some((ContainerKind::Inventory, None)),
        ContainerTarget::Table => Some((ContainerKind::CraftingTable, None)),
        ContainerTarget::Furnace(pos) => Some((ContainerKind::Furnace, Some(pos))),
        ContainerTarget::Chest(pos) => Some((ContainerKind::Chest, Some(pos))),
        ContainerTarget::FurnitureWorkbench => Some((ContainerKind::FurnitureWorkbench, None)),
        ContainerTarget::ModGui { kind, pos } => Some((ContainerKind::Mod(kind), pos)),
    }
}
