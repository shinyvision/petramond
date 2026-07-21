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
use crate::crafting::CraftingStation;
use crate::events::{PostEvent, SimCtx};
use crate::gui::{ChestView, ContainerView, FurnaceView, GuiStateMap, MenuSlot};
use crate::inventory::Inventory;
use crate::item::ItemStack;
use crate::mathh::IVec3;
use crate::net::protocol::{ItemSlotWire, MenuSyncMsg, MenuTargetWire};

use super::game::ServerGame;
use crate::game::container::{ContainerTarget, CraftMenuFailure};
use crate::game::tick::TickEvents;
use crate::net::protocol::ActionDenyReason;
use crate::server::player::PendingMenuAction;

/// Read-only menu state consumed by the app's UI snapshot builder. The
/// CLIENT assembles this entirely from its replicated stores
/// (`SelfView.inventory` + the `MenuView` fed by `MenuSyncMsg`) — see
/// `Game::menu_read_model`; nothing here reads a server session.
pub struct MenuReadModel<'a> {
    pub inventory: &'a Inventory,
    pub craft_output: Option<ItemStack>,
    pub furnace: Option<FurnaceView>,
    pub chest: Option<ChestView>,
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
            self.tick_menu(s, &mut events);
            self.tick_drops(s, &mut events);
        }
    }

    /// Apply this frame's ordered menu actions on the tick. Splits the
    /// disjoint `menu` / `world` / `inventory` borrows the menu needs and lends the
    /// recipes; the menu decodes each interaction keyed on its target. Widget
    /// (button) clicks mutate no container — they dispatch to the open mod
    /// GUI's owning mod instead. Keeping transitions and mutations in one
    /// stream prevents close/click/craft races.
    pub(crate) fn tick_menu(&mut self, s: usize, events: &mut TickEvents) {
        for action in std::mem::take(&mut self.sessions[s].pending_menu_actions) {
            match action {
                PendingMenuAction::OpenGui { kind, pos } => {
                    self.replace_open_menu_for(s, events);
                    if self.open_gui_for(s, kind, pos, events) {
                        self.sessions[s].request_open_gui = Some((kind, pos));
                    }
                }
                PendingMenuAction::Close => self.close_open_menu_for(s, events),
                PendingMenuAction::SlotClick {
                    slot,
                    button,
                    shift,
                    gather,
                    request_id,
                } => {
                    if let MenuSlot::Widget(id) = slot {
                        // A right-click is consumed by the button but does not
                        // activate it.
                        if button == PointerButton::Primary {
                            self.dispatch_gui_click(s, id, events);
                        }
                    } else {
                        let sess = &mut self.sessions[s];
                        sess.menu.click(
                            &mut self.world,
                            &mut sess.player.inventory,
                            slot,
                            button,
                            shift,
                            gather,
                        );
                        // A click may be predicted across both client
                        // mirrors. Force the authoritative pair into the
                        // outcome batch even if a stale client mirror made
                        // the server-side action a no-op and neither
                        // ordinary on-change gate moved — the client skips
                        // interim snapshots while its prediction is pending
                        // and reconciles from exactly this batch.
                        sess.last_sent_inventory_revision = None;
                        sess.last_menu_sync = None;
                    }
                    self.push_action_outcome(s, request_id, true, None);
                }
                PendingMenuAction::SlotDrag {
                    slots,
                    button,
                    request_id,
                } => {
                    let sess = &mut self.sessions[s];
                    sess.menu.drag_slots(
                        &mut self.world,
                        &mut sess.player.inventory,
                        &slots,
                        button,
                    );
                    // A drag is predicted across both client mirrors. Force
                    // the authoritative pair into the outcome batch even if
                    // stale client capacity made the server-side action a
                    // no-op and neither ordinary on-change gate moved.
                    sess.last_sent_inventory_revision = None;
                    sess.last_menu_sync = None;
                    self.push_action_outcome(s, request_id, true, None);
                }
                PendingMenuAction::DropSlot {
                    slot,
                    all,
                    request_id,
                } => {
                    let dropped = {
                        let sess = &mut self.sessions[s];
                        sess.menu.drop_slot(
                            &mut self.world,
                            &mut sess.player.inventory,
                            slot,
                            all,
                        )
                    };
                    if let Some(stack) = dropped {
                        self.sessions[s].drop_queue.queue_stack(stack);
                    }
                    self.push_action_outcome(
                        s,
                        request_id,
                        dropped.is_some(),
                        dropped.is_none().then_some(ActionDenyReason::Denied),
                    );
                }
                PendingMenuAction::CraftRecipe {
                    recipe,
                    bulk,
                    request_id,
                } => {
                    let result = {
                        let sess = &mut self.sessions[s];
                        sess.menu.craft_recipe(
                            &mut sess.player.inventory,
                            &self.recipes,
                            &recipe,
                            bulk,
                        )
                    };
                    match result {
                        Ok(overflow) => {
                            for stack in overflow {
                                self.sessions[s].drop_queue.queue_stack(stack);
                            }
                            self.push_action_outcome(s, request_id, true, None);
                        }
                        Err(error) => {
                            let reason = match error {
                                CraftMenuFailure::InvalidRecipe => ActionDenyReason::InvalidSlot,
                                CraftMenuFailure::OutputOccupied => ActionDenyReason::Busy,
                                CraftMenuFailure::MissingIngredients => ActionDenyReason::Denied,
                            };
                            self.push_action_outcome(s, request_id, false, Some(reason));
                        }
                    }
                }
            }
        }
    }

    /// Replace an existing menu through the ordinary close funnel before a
    /// new target opens. Transient cursor/output stacks are thereby
    /// recovered exactly once, and chest viewer state cannot leak across a
    /// direct menu transition.
    fn replace_open_menu_for(&mut self, s: usize, events: &mut TickEvents) {
        if self.sessions[s].menu.target() != ContainerTarget::None {
            self.close_open_menu_for(s, events);
        } else {
            self.clear_menu_open_requests(s);
        }
    }

    fn clear_menu_open_requests(&mut self, s: usize) {
        self.sessions[s].request_open_gui = None;
    }

    /// Begin session `s`'s GUI session for `kind`, opened from block `pos`
    /// (`None` for the inventory key / a programmatic `GuiOpen`). The ONE
    /// open dispatch: every kind — engine container or mod GUI — arrives
    /// through the same `OpenGui` action, and the per-kind session setup
    /// (crafting station, chest viewer slot, mod GUI state clear) keys on the
    /// kind here. Returns whether a session actually opened (a block-entity
    /// kind without a position, or a shell kind, opens nothing).
    fn open_gui_for(
        &mut self,
        s: usize,
        kind: crate::gui::GuiKind,
        pos: Option<IVec3>,
        events: &mut TickEvents,
    ) -> bool {
        use crate::gui::GuiKind;
        // Any registered crafting station — the engine pair or a pack
        // workbench kind — opens the ordinary crafting session, never a mod
        // GUI session.
        if let Some(station) = CraftingStation::of_kind(kind) {
            self.open_crafting_for(s, station);
            return true;
        }
        match kind {
            GuiKind::Furnace => {
                let Some(pos) = pos else { return false };
                self.open_furnace_screen_for(s, pos);
            }
            GuiKind::Chest => {
                let Some(pos) = pos else { return false };
                self.open_chest_screen_for(s, pos, events);
            }
            kind if kind.is_mod() => self.open_mod_gui_screen_for(s, kind, pos),
            _ => return false,
        }
        true
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
        let ContainerTarget::Gui { kind, pos } = self.sessions[s].menu.target() else {
            return;
        };
        // Engine kinds have no owning mod; their buttons are documented dead
        // ends, exactly like a content-only pack's. Station sessions are
        // engine-driven even under a pack kind — their buttons belong to the
        // client crafting browser, never to a mod dispatch.
        if !kind.is_mod() || CraftingStation::of_kind(kind).is_some() {
            return;
        }
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
        mods.dispatch_gui_click(&mut ctx, kind_key, widget_id, pos.map(|p| p.to_array()));
    }

    /// Begin a fresh player-crafting session for the requested station.
    pub(crate) fn open_crafting_for(&mut self, s: usize, station: CraftingStation) {
        let sess = &mut self.sessions[s];
        sess.menu.open_crafting(station);
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
            ContainerTarget::Gui { kind: crate::gui::GuiKind::Chest, pos: Some(p) } if p == pos
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
        if let ContainerTarget::Gui {
            kind: crate::gui::GuiKind::Chest,
            pos: Some(pos),
        } = self.sessions[s].menu.target()
        {
            if let Some(count) = self.chest_viewers.get_mut(&pos) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.chest_viewers.remove(&pos);
                    events.world.chest_changed.push((pos, false));
                }
            }
        }
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
    /// cursor stack, player-crafting output, furnace, chest, then any mod
    /// GUI (whose session state map is cleared with it).
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
        self.close_mod_gui_for(s);
        self.clear_menu_open_requests(s);
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

    /// Return the real player-crafting output to the inventory, queueing any
    /// overflow for the ordinary world-drop stage.
    fn close_crafting_for(&mut self, s: usize) {
        let mut overflow = Vec::new();
        let sess = &mut self.sessions[s];
        sess.menu
            .close_crafting(&mut sess.player.inventory, |stack| overflow.push(stack));
        for stack in overflow {
            sess.drop_queue.queue_stack(stack);
        }
    }

    /// End the mod GUI session and clear its state map.
    fn close_mod_gui_for(&mut self, s: usize) {
        if self.sessions[s]
            .menu
            .target()
            .kind()
            .is_some_and(|kind| kind.is_mod())
        {
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
            .any(|sess| sess.menu.target().kind().is_some_and(|kind| kind.is_mod()))
    }

    fn clear_all_mod_gui_states(&mut self) {
        for sess in &mut self.sessions {
            crate::gui::gui_state_clear(&mut sess.gui_state);
            sess.last_sent_gui_state = None;
        }
    }

    /// Session `s`'s menu view as the wire message, with `gui_state` held
    /// `None` (the caller attaches the map only when its `Arc` changed).
    pub(super) fn build_menu_sync_base(&self, s: usize) -> MenuSyncMsg {
        use crate::gui::GuiKind;
        let sess = &self.sessions[s];
        // The wire keeps per-kind view payloads (gauges, chest slots);
        // this is the one kind-keyed lookup that selects them.
        let target = match sess.menu.target() {
            ContainerTarget::None => MenuTargetWire::None,
            ContainerTarget::Gui { kind, pos } => match kind {
                kind if CraftingStation::of_kind(kind).is_some() => MenuTargetWire::Crafting {
                    output: slot_wire(sess.menu.craft_output()),
                },
                GuiKind::Furnace => {
                    let v = sess.menu.open_furnace_view(&self.world).unwrap_or_default();
                    MenuTargetWire::Furnace {
                        pos: pos.unwrap_or_default(),
                        slots: [slot_wire(v.input), slot_wire(v.fuel), slot_wire(v.output)],
                        cook01: v.cook01,
                        burn01: v.burn01,
                    }
                }
                GuiKind::Chest => {
                    let slots = sess
                        .menu
                        .open_chest_view(&self.world)
                        .map(|v| v.slots.iter().map(|s| slot_wire(*s)).collect())
                        .unwrap_or_default();
                    MenuTargetWire::Chest {
                        pos: pos.unwrap_or_default(),
                        slots,
                    }
                }
                kind => MenuTargetWire::ModGui {
                    kind_key: crate::gui::kind_key(kind).unwrap_or_default().to_string(),
                    pos,
                    slots: sess
                        .menu
                        .open_container_view(&self.world)
                        .map(|v| v.slots.iter().map(|s| slot_wire(*s)).collect()),
                    gui_state: None,
                },
            },
        };
        MenuSyncMsg { target }
    }
}

/// The `container_opened`/`container_closed` payload for a menu target, or `None`
/// when no container session is involved. The unified target already carries
/// the event's `(kind, pos)` identity.
fn container_event_key(target: ContainerTarget) -> Option<(crate::gui::GuiKind, Option<IVec3>)> {
    match target {
        ContainerTarget::None => None,
        ContainerTarget::Gui { kind, pos } => Some((kind, pos)),
    }
}
