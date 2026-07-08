//! Server-side menu/session facade.
//!
//! `ContainerMenu` owns low-level slot behavior in `game/container`. This module
//! owns the `ServerGame` boundary around that menu: opening edit targets ON THE
//! TICK (from the interaction/mod-action request sites), buffering menu clicks
//! for fixed ticks, close-session cleanup, and the per-session `MenuSyncMsg`
//! the replication batch ships. Each player session owns its own
//! `ContainerMenu` — two players can stand in one chest; their clicks apply in
//! session-id order on the tick.

use crate::controls::PointerButton;
use crate::crafting::CraftGrid;
use crate::events::{ContainerKind, PostEvent, SimCtx};
use crate::gui::{ChestView, ContainerView, FurnaceView, GuiStateMap, MenuSlot, WorkbenchView};
use crate::inventory::Inventory;
use crate::item::ItemStack;
use crate::mathh::IVec3;
use crate::net::protocol::{ItemSlotWire, MenuSyncMsg, MenuTargetWire};

use super::game::ServerGame;
use crate::game::container::ContainerTarget;
use crate::game::tick::TickEvents;

/// Read-only menu state consumed by the app's UI snapshot builder. Since
/// C2c-iii the CLIENT assembles this entirely from its replicated stores
/// (`SelfView.inventory` + the `MenuView` fed by `MenuSyncMsg`) — see
/// `Game::menu_read_model`; nothing here reads a server session.
pub struct MenuReadModel<'a> {
    pub inventory: &'a Inventory,
    pub craft: &'a CraftGrid,
    pub furnace: Option<FurnaceView>,
    pub chest: Option<ChestView>,
    pub workbench: Option<WorkbenchView>,
    /// The open mod GUI's state map (a shared snapshot), or `None` when the
    /// open session is not a mod GUI.
    pub gui_state: Option<std::sync::Arc<GuiStateMap>>,
    /// The open mod GUI's container slots, or `None` when the session is not
    /// a slot-bearing mod GUI.
    pub container: Option<ContainerView>,
}

fn slot_wire(slot: Option<ItemStack>) -> Option<ItemSlotWire> {
    slot.map(|s| ItemSlotWire {
        item_id: s.item.0,
        count: s.count,
    })
}

impl ServerGame {
    /// Apply the player actions latched this frame — container edits and item drops — at
    /// once, standing in for the game tick that resolves them in play. For App-level tests
    /// that drive the input routing and then assert the resulting inventory / world state
    /// (between two clicks a real tick interleaves, applying the first before the second is
    /// decided — call this there too).
    #[cfg(test)]
    pub(crate) fn apply_latched_actions_for_test(&mut self) {
        let mut events = TickEvents::default();
        for s in 0..self.sessions.len() {
            if std::mem::take(&mut self.sessions[s].close_menu_requested) {
                self.close_open_menu_for(s, &mut events);
            }
        }
        self.tick_drops(0, &mut events);
        self.tick_menu(0, &mut events);
    }

    /// Apply this frame's latched container-menu clicks, in order, on the tick. Splits the
    /// disjoint `menu` / `world` / `inventory` borrows the menu needs and lends the
    /// recipes; the menu decodes each interaction keyed on its target. Widget
    /// (button) clicks mutate no container — they dispatch to the open mod
    /// GUI's owning mod instead. A latched `OpenInventory` opens the 2×2
    /// crafting session FIRST, so clicks queued behind the open land in it.
    pub(crate) fn tick_menu(&mut self, s: usize, events: &mut TickEvents) {
        if std::mem::take(&mut self.sessions[s].open_inventory_requested) {
            self.open_crafting_for(s, 2);
            self.sessions[s].request_open_inventory = true;
        }
        for (slot, button, shift, gather) in
            std::mem::take(&mut self.sessions[s].pending_menu_clicks)
        {
            if let MenuSlot::Widget(id) = slot {
                // Only a primary click activates a button (a right-click over
                // one is consumed by the panel but triggers nothing).
                if button == PointerButton::Primary {
                    self.dispatch_gui_click(s, id, events);
                }
                continue;
            }
            let sess = &mut self.sessions[s];
            sess.menu.click(
                &mut self.world,
                &mut sess.player.inventory,
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
    fn dispatch_gui_click(
        &mut self,
        s: usize,
        widget_id: crate::gui::WidgetId,
        events: &mut TickEvents,
    ) {
        let ContainerTarget::ModGui { kind, pos } = self.sessions[s].menu.target() else {
            return;
        };
        let Some(kind_key) = crate::gui::kind_key(kind) else {
            return;
        };
        let Self {
            world,
            sessions,
            bus,
            mods,
            ..
        } = self;
        let sess = &mut sessions[s];
        let mut ctx = SimCtx {
            world,
            player: &mut sess.player,
            gui_state: &mut sess.gui_state,
            feed: events,
            queue: bus.queue_mut(),
        };
        mods.dispatch_gui_click(&mut ctx, kind_key, widget_id, pos.map(|p| [p.x, p.y, p.z]));
    }

    /// Configure session `s`'s crafting grid for a screen of `cols×cols`
    /// (2 = inventory, 3 = table) and clear it. Runs on the tick, at the
    /// request site (interaction arm / `OpenInventory` latch).
    pub(crate) fn open_crafting_for(&mut self, s: usize, cols: usize) {
        let sess = &mut self.sessions[s];
        sess.menu.open_crafting(cols, &self.recipes);
        self.emit_container_opened(s);
    }

    /// Begin session `s`'s furnace-screen session at `pos`.
    pub(crate) fn open_furnace_screen_for(&mut self, s: usize, pos: IVec3) {
        let sess = &mut self.sessions[s];
        sess.menu.open_furnace_screen(&mut self.world, pos);
        self.emit_container_opened(s);
    }

    /// Begin session `s`'s chest-screen session at `pos`. A 0→1 viewer
    /// transition emits the world-anchored `ChestOpened` event.
    pub(crate) fn open_chest_screen_for(&mut self, s: usize, pos: IVec3, events: &mut TickEvents) {
        // Re-opening the SAME chest keeps the held viewer slot (no leak, and
        // no phantom close→open transition events); a different chest first
        // releases the old slot.
        let same = matches!(
            self.sessions[s].menu.target(),
            ContainerTarget::Chest(p) if p == pos
        );
        if !same {
            self.release_chest_viewer(s, events);
        }
        let sess = &mut self.sessions[s];
        sess.menu.open_chest_screen(&mut self.world, pos);
        if !same {
            let count = self.chest_viewers.entry(pos).or_insert(0);
            *count += 1;
            if *count == 1 {
                events.world.chest_changed.push((pos, true));
            }
        }
        self.emit_container_opened(s);
    }

    /// Release player `s`'s viewer slot on whatever chest their menu targets.
    /// The lid falls (for every observer) only when the LAST viewer leaves —
    /// that 1→0 transition emits the world-anchored `ChestClosed` event.
    fn release_chest_viewer(&mut self, s: usize, events: &mut TickEvents) {
        if let ContainerTarget::Chest(pos) = self.sessions[s].menu.target() {
            if let Some(count) = self.chest_viewers.get_mut(&pos) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.chest_viewers.remove(&pos);
                    events.world.chest_changed.push((pos, false));
                }
            }
        }
    }

    /// Begin session `s`'s furniture-workbench session (input starts empty).
    pub(crate) fn open_workbench_screen_for(&mut self, s: usize) {
        self.sessions[s].menu.open_workbench();
        self.emit_container_opened(s);
    }

    /// Begin session `s`'s mod GUI session for `kind`, opened from block
    /// `pos` (`None` for a programmatic `GuiOpen`). The session's state map
    /// starts empty — cleared here so no session can read a predecessor's
    /// values.
    pub(crate) fn open_mod_gui_screen_for(
        &mut self,
        s: usize,
        kind: crate::gui::GuiKind,
        pos: Option<IVec3>,
    ) {
        if !self.any_mod_gui_open() {
            self.clear_all_mod_gui_states();
        }
        let sess = &mut self.sessions[s];
        crate::gui::gui_state_clear(&mut sess.gui_state);
        sess.menu.open_mod_gui(&mut self.world, kind, pos);
        self.emit_container_opened(s);
    }

    /// Close player `s`'s open menu session in the app-required cleanup order:
    /// cursor stack, crafting grid, furnace, chest, furniture workbench, then
    /// any mod GUI (whose session state map is cleared with it).
    pub(crate) fn close_open_menu_for(&mut self, s: usize, events: &mut TickEvents) {
        // `container_closed` for whatever session was actually open. Emitted
        // (not dispatched) here: the handler runs at the tick's next drain
        // point, like every queued event.
        if let Some((kind, pos)) = container_event_key(self.sessions[s].menu.target()) {
            self.bus.emit(PostEvent::ContainerClosed { kind, pos });
        }
        self.release_chest_viewer(s, events);
        self.close_cursor_stack_for(s);
        self.close_crafting_for(s);
        self.sessions[s].menu.close_furnace();
        self.sessions[s].menu.close_chest();
        self.close_workbench_for(s);
        self.close_mod_gui_for(s);
    }

    /// `container_opened` for the session that just began. The `open_*_for`
    /// methods are the single funnel every container screen opens through
    /// (whether from a block interact, a mod action, or the inventory key),
    /// so the event fires exactly once per session.
    fn emit_container_opened(&mut self, s: usize) {
        if let Some((kind, pos)) = container_event_key(self.sessions[s].menu.target()) {
            self.bus.emit(PostEvent::ContainerOpened { kind, pos });
        }
    }

    /// Close the crafting grid: return every input item to the inventory (any
    /// overflow is thrown into the world), then clear the result. Overflow is
    /// gathered first, then queued after the menu call so the drop spawn path can
    /// borrow `world` later on the fixed tick.
    fn close_crafting_for(&mut self, s: usize) {
        let mut overflow = Vec::new();
        let sess = &mut self.sessions[s];
        sess.menu
            .close_crafting(&mut sess.player.inventory, &self.recipes, |stack| {
                overflow.push(stack);
            });
        for stack in overflow {
            sess.drop_queue.queue_stack(stack);
        }
    }

    /// End the mod GUI session and clear its state map.
    fn close_mod_gui_for(&mut self, s: usize) {
        if matches!(
            self.sessions[s].menu.target(),
            ContainerTarget::ModGui { .. }
        ) {
            let sess = &mut self.sessions[s];
            crate::gui::gui_state_clear(&mut sess.gui_state);
            sess.menu.close_mod_gui();
            if !self.any_mod_gui_open() {
                self.clear_all_mod_gui_states();
            }
        }
    }

    fn any_mod_gui_open(&self) -> bool {
        self.sessions
            .iter()
            .any(|sess| matches!(sess.menu.target(), ContainerTarget::ModGui { .. }))
    }

    fn clear_all_mod_gui_states(&mut self) {
        for sess in &mut self.sessions {
            crate::gui::gui_state_clear(&mut sess.gui_state);
            sess.last_sent_gui_state = None;
        }
    }

    /// End the workbench session: return the input block to the inventory (overflow
    /// thrown into the world), like closing the crafting grid.
    fn close_workbench_for(&mut self, s: usize) {
        let mut overflow = Vec::new();
        let sess = &mut self.sessions[s];
        sess.menu
            .close_workbench(&mut sess.player.inventory, |stack| overflow.push(stack));
        for stack in overflow {
            sess.drop_queue.queue_stack(stack);
        }
    }

    /// Session `s`'s menu view as the wire message, with `gui_state` held
    /// `None` (the caller attaches the map only when its `Arc` changed).
    pub(super) fn build_menu_sync_base(&self, s: usize) -> MenuSyncMsg {
        let sess = &self.sessions[s];
        let target = match sess.menu.target() {
            ContainerTarget::None => MenuTargetWire::None,
            ContainerTarget::Inventory => MenuTargetWire::Inventory,
            ContainerTarget::Table => MenuTargetWire::Table,
            ContainerTarget::Furnace(pos) => {
                let v = sess
                    .menu
                    .open_furnace_view(&self.world)
                    .unwrap_or_default();
                MenuTargetWire::Furnace {
                    pos,
                    slots: [slot_wire(v.input), slot_wire(v.fuel), slot_wire(v.output)],
                    cook01: v.cook01,
                    burn01: v.burn01,
                }
            }
            ContainerTarget::Chest(pos) => {
                let slots = sess
                    .menu
                    .open_chest_view(&self.world)
                    .map(|v| v.slots.iter().map(|s| slot_wire(*s)).collect())
                    .unwrap_or_default();
                MenuTargetWire::Chest { pos, slots }
            }
            ContainerTarget::FurnitureWorkbench => {
                let v = sess
                    .menu
                    .open_workbench_view(&self.recipes)
                    .unwrap_or_default();
                MenuTargetWire::Workbench {
                    input: slot_wire(v.input),
                    results: v.results.iter().map(|&(item, ok)| (item.0, ok)).collect(),
                }
            }
            ContainerTarget::ModGui { kind, pos } => MenuTargetWire::ModGui {
                kind_key: crate::gui::kind_key(kind).unwrap_or_default().to_string(),
                pos,
                slots: sess
                    .menu
                    .open_container_view(&self.world)
                    .map(|v| v.slots.iter().map(|s| slot_wire(*s)).collect()),
                gui_state: None,
            },
        };
        let craft = sess.menu.craft_grid();
        MenuSyncMsg {
            target,
            craft_grid: craft.cells()[..craft.capacity()]
                .iter()
                .map(|c| slot_wire(*c))
                .collect(),
            craft_result: slot_wire(craft.result().copied()),
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
