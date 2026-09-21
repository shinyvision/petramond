//! Server-side menu/session facade.
//!
//! `ContainerMenu` owns low-level slot behavior in `game/container`. This module
//! owns the `ServerGame` boundary around that menu: opening edit targets ON THE
//! TICK (from the interaction/mod-action request sites), buffering menu clicks
//! for fixed ticks, close-session cleanup, and the per-session `MenuSyncMsg`
//! the replication batch ships. Each player session owns its own
//! `ContainerMenu` — two players can stand in one chest; their clicks apply in
//! session-id order on the tick.

use crate::events::{PostEvent, SimCtx};
use crate::net::protocol::{GuiValueWire, ItemSlotWire, MenuSyncMsg, MenuTargetWire};
use petramond_math::math::IVec3;
use petramond_world::crafting::CraftingStation;
use petramond_world::gui_state::ContainerView;
use petramond_world::gui_state::PointerButton;
use petramond_world::gui_state::{GuiStateMap, MenuSlot};
use petramond_world::inventory::Inventory;
use petramond_world::item::ItemStack;

use super::game::ServerGame;
use crate::events::tick::TickEvents;
use crate::menu::{ContainerTarget, CraftMenuFailure, MenuAnchor};
use crate::net::protocol::ActionDenyReason;
use crate::server::player::PendingMenuAction;

/// Read-only menu state consumed by the app's UI snapshot builder. The
/// CLIENT assembles this entirely from its replicated stores
/// (`SelfView.inventory` + the `MenuView` fed by `MenuSyncMsg`) — see
/// `Game::menu_read_model`; nothing here reads a server session.
pub struct MenuReadModel<'a> {
    pub inventory: &'a Inventory,
    pub craft_output: Option<ItemStack>,
    /// The open mod GUI's state map (a shared snapshot), or `None` when the
    /// open session is not a mod GUI.
    pub gui_state: Option<std::sync::Arc<GuiStateMap>>,
    /// The open mod GUI's container slots, or `None` when the session is not
    /// a slot-bearing mod GUI.
    pub container: Option<ContainerView>,
}

fn slot_wire(slot: Option<ItemStack>) -> Option<ItemSlotWire> {
    slot.map(ItemSlotWire::from_stack)
}

impl ServerGame {
    /// Apply the player actions latched this frame — container edits and item drops — at
    /// once, standing in for the game tick that resolves them in play. For App-level tests
    /// that drive the input routing and then assert the resulting inventory / world state
    /// (between two clicks a real tick interleaves, applying the first before the second is
    /// decided — call this there too).
    #[cfg(any(test, feature = "test-support"))]
    pub fn apply_latched_actions_for_test(&mut self) {
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
    pub fn tick_menu(&mut self, s: usize, events: &mut TickEvents) {
        for action in std::mem::take(&mut self.sessions[s].pending_menu_actions) {
            match action {
                PendingMenuAction::OpenGui { kind, anchor } => {
                    self.replace_open_menu_for(s, events);
                    if self.open_gui_for(s, kind, anchor, events) {
                        self.sessions[s].request_open_gui = Some((kind, anchor));
                    }
                }
                PendingMenuAction::Close => self.close_open_menu_for(s, events),
                PendingMenuAction::CreativeCursor { item, request_id } => {
                    use petramond_world::{
                        gui_state::GuiKind,
                        item::{ItemStack, ItemType},
                    };
                    let allowed = self.is_operator(s)
                        && self.sessions[s].player.abilities().item_catalog
                        && self.sessions[s].menu.target().kind() == Some(GuiKind::Creative);
                    let resolved = item
                        .as_deref()
                        .and_then(ItemType::by_name)
                        .filter(|i| i.creative_visible());
                    let accepted = allowed && (item.is_none() || resolved.is_some());
                    if accepted {
                        *self.sessions[s].player.inventory.cursor_mut() =
                            resolved.map(|i| ItemStack::new(i, i.max_stack_size()));
                    }
                    self.sessions[s].last_sent_inventory_revision = None;
                    self.sessions[s].last_menu_sync = None;
                    self.push_action_outcome(
                        s,
                        request_id,
                        accepted,
                        (!accepted).then_some(ActionDenyReason::Denied),
                    );
                }
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
                        let gui = sess.gui_state.clone();
                        sess.menu.click(
                            &mut self.world,
                            &mut sess.player.inventory,
                            Some(&gui),
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
                    let gui = sess.gui_state.clone();
                    sess.menu.drag_slots(
                        &mut self.world,
                        &mut sess.player.inventory,
                        Some(&gui),
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
                        sess.menu
                            .drop_slot(&mut self.world, &mut sess.player.inventory, slot, all)
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
                PendingMenuAction::SwapOffHand { slot, request_id } => {
                    let accepted = !self.sessions[s].player.is_spectator();
                    if accepted {
                        let sess = &mut self.sessions[s];
                        let gui = sess.gui_state.clone();
                        sess.menu.swap_off_hand(
                            &mut self.world,
                            &mut sess.player.inventory,
                            Some(&gui),
                            slot,
                        );
                        // The swap is predicted across both client mirrors;
                        // force the authoritative pair into the outcome batch
                        // so the pending prediction reconciles from it (the
                        // SlotClick rule).
                        sess.last_sent_inventory_revision = None;
                        sess.last_menu_sync = None;
                    }
                    self.push_action_outcome(
                        s,
                        request_id,
                        accepted,
                        (!accepted).then_some(ActionDenyReason::Denied),
                    );
                }
                PendingMenuAction::CraftRecipe {
                    recipe,
                    bulk,
                    request_id,
                } => {
                    let result = {
                        let sess = &mut self.sessions[s];
                        let (player, menu) = (&mut sess.player, &mut sess.menu);
                        menu.craft_recipe(
                            &mut player.inventory,
                            &self.recipes,
                            &player.progression,
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

    /// Begin session `s`'s GUI session for `kind`, opened on `anchor`
    /// (`None` for the inventory key / an unanchored `GuiOpen`). The ONE
    /// open dispatch: every kind — engine container or mod GUI — arrives
    /// through the same `OpenGui` action, and the per-kind session setup
    /// (crafting station, chest viewer slot, mod GUI state clear) keys on the
    /// kind here. Returns whether a session actually opened (a block-entity
    /// kind without a block, an anchor that is no longer there, or a shell
    /// kind, opens nothing).
    fn open_gui_for(
        &mut self,
        s: usize,
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<MenuAnchor>,
        events: &mut TickEvents,
    ) -> bool {
        let pos = anchor.and_then(MenuAnchor::block);
        // Any registered crafting station — the engine pair or a pack
        // workbench kind — opens the ordinary crafting session, never a mod
        // GUI session.
        if let Some(station) = CraftingStation::of_kind(kind) {
            self.open_crafting_for(s, station);
            return true;
        }
        use petramond_world::gui_state::GuiKind;
        match kind {
            GuiKind::Creative if self.sessions[s].player.abilities().item_catalog => {
                self.sessions[s]
                    .menu
                    .open_document_gui(&mut self.world, kind, None);
                self.emit_container_opened(s);
            }
            GuiKind::Furnace => {
                let Some(pos) = pos else { return false };
                self.open_furnace_screen_for(s, pos);
            }
            GuiKind::Chest => {
                let Some(pos) = pos else { return false };
                self.open_chest_screen_for(s, pos, events);
            }
            kind if kind.is_registered() => {
                if anchor.is_some_and(|anchor| !anchor.present(&self.world)) {
                    return false;
                }
                self.open_registered_gui_screen_for(s, kind, anchor)
            }
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
        widget_id: petramond_world::gui_state::WidgetId,
        events: &mut TickEvents,
    ) {
        let ContainerTarget::Gui { kind, anchor } = self.sessions[s].menu.target() else {
            return;
        };
        // Engine kinds have no owning mod; their buttons are documented dead
        // ends, exactly like a content-only pack's. Station sessions are
        // engine-driven even under a pack kind — their buttons belong to the
        // client crafting browser, never to a mod dispatch.
        if !kind.is_registered() || CraftingStation::of_kind(kind).is_some() {
            return;
        }
        let Some(kind_key) = petramond_world::gui_state::kind_key(kind) else {
            return;
        };
        let Self {
            world,
            sessions,
            bus,
            mods,
            ..
        } = self;
        Self::with_sessions_view(sessions, s, |sess| {
            let mut ctx = SimCtx {
                world,
                player: &mut sess.player,
                gui_state: &mut sess.gui_state,
                feed: events,
                queue: bus.queue_mut(),
            };
            mods.dispatch_gui_click(&mut ctx, kind_key, widget_id, anchor);
        });
    }

    /// Begin a fresh player-crafting session for the requested station.
    pub fn open_crafting_for(&mut self, s: usize, station: CraftingStation) {
        let sess = &mut self.sessions[s];
        sess.menu.open_crafting(station);
        self.emit_container_opened(s);
    }

    /// Begin session `s`'s furnace-screen session at `pos`.
    pub fn open_furnace_screen_for(&mut self, s: usize, pos: IVec3) {
        let sess = &mut self.sessions[s];
        sess.menu.open_furnace_screen(&mut self.world, pos);
        self.emit_container_opened(s);
    }

    /// Begin session `s`'s chest-screen session at `pos`. A 0→1 viewer
    /// transition emits the world-anchored `ChestOpened` event.
    pub fn open_chest_screen_for(&mut self, s: usize, pos: IVec3, events: &mut TickEvents) {
        // Re-opening the SAME chest keeps the held viewer slot (no leak, and
        // no phantom close→open transition events); a different chest first
        // releases the old slot.
        let same = matches!(
            self.sessions[s].menu.target(),
            ContainerTarget::Gui { kind: petramond_world::gui_state::GuiKind::Chest, anchor: Some(MenuAnchor::Block(p)) } if p == pos
        );
        if !same {
            self.release_chest_viewer(s, events);
        }
        let sess = &mut self.sessions[s];
        sess.menu.open_chest_screen(&mut self.world, pos);
        if !same {
            self.add_chest_viewer(pos, events);
        }
        self.emit_container_opened(s);
    }

    /// One more viewer of the chest at `pos`; the first lifts its lid.
    fn add_chest_viewer(&mut self, pos: IVec3, events: &mut TickEvents) {
        let count = self.chest_viewers.entry(pos).or_insert(0);
        *count += 1;
        if *count == 1 {
            events.world.chest_changed.push((pos, true));
        }
    }

    /// One viewer fewer of the chest at `pos`; the last lets its lid fall.
    fn drop_chest_viewer(&mut self, pos: IVec3, events: &mut TickEvents) {
        if let Some(count) = self.chest_viewers.get_mut(&pos) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.chest_viewers.remove(&pos);
                events.world.chest_changed.push((pos, false));
            }
        }
    }

    /// A mob holding the container at `pos` open, or letting it go
    /// (`ContainerHold`). Only a chest shows it: the mob counts once among
    /// its viewers, as a player's open screen does.
    pub(super) fn hold_container(
        &mut self,
        mob_id: u64,
        pos: IVec3,
        open: bool,
        events: &mut TickEvents,
    ) {
        let holders = self.container_holds.entry(pos).or_default();
        let held = holders.contains(&mob_id);
        if open && !held && self.world.mobs().index_of_id(mob_id).is_some() {
            holders.push(mob_id);
            let chest = self
                .world
                .block_if_stream_final(pos.x, pos.y, pos.z)
                .is_some_and(|b| {
                    b.interaction()
                        == petramond_world::block::BlockInteraction::OpenGui(
                            petramond_world::gui_state::GuiKind::Chest,
                        )
                });
            if chest {
                self.add_chest_viewer(pos, events);
            }
        } else if !open && held {
            holders.retain(|id| *id != mob_id);
            if holders.is_empty() {
                self.container_holds.remove(&pos);
            }
            self.drop_chest_viewer(pos, events);
        }
    }

    /// Let go every container held by a mob no longer in the world.
    pub(super) fn release_absent_holders(&mut self, events: &mut TickEvents) {
        if self.container_holds.is_empty() {
            return;
        }
        let gone: Vec<(IVec3, u64)> = self
            .container_holds
            .iter()
            .flat_map(|(pos, ids)| ids.iter().map(move |id| (*pos, *id)))
            .filter(|(_, id)| self.world.mobs().index_of_id(*id).is_none())
            .collect();
        for (pos, id) in gone {
            self.hold_container(id, pos, false, events);
        }
    }

    /// Release player `s`'s viewer slot on whatever chest their menu targets.
    /// The lid falls (for every observer) only when the LAST viewer leaves —
    /// that 1→0 transition emits the world-anchored `ChestClosed` event.
    fn release_chest_viewer(&mut self, s: usize, events: &mut TickEvents) {
        if let ContainerTarget::Gui {
            kind: petramond_world::gui_state::GuiKind::Chest,
            anchor: Some(MenuAnchor::Block(pos)),
        } = self.sessions[s].menu.target()
        {
            self.drop_chest_viewer(pos, events);
        }
    }

    /// Begin session `s`'s mod GUI session for `kind`, opened on `anchor`
    /// (`None` for an unanchored `GuiOpen`). The session's state map
    /// starts empty — cleared here so no session can read a predecessor's
    /// values.
    pub fn open_registered_gui_screen_for(
        &mut self,
        s: usize,
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<MenuAnchor>,
    ) {
        if !self.any_registered_gui_open() {
            self.clear_all_gui_states();
        }
        let sess = &mut self.sessions[s];
        petramond_world::gui_state::gui_state_clear(&mut sess.gui_state);
        sess.menu.open_document_gui(&mut self.world, kind, anchor);
        self.emit_container_opened(s);
    }

    /// End every session whose anchor left the world (a mob that died,
    /// despawned or unloaded), through the ordinary close funnel, and tell
    /// the client to drop the screen.
    pub(super) fn close_menus_on_absent_anchors(&mut self, events: &mut TickEvents) {
        for s in 0..self.sessions.len() {
            let gone = self.sessions[s]
                .menu
                .target()
                .anchor()
                .is_some_and(|anchor| !anchor.present(&self.world));
            if gone {
                self.close_open_menu_for(s, events);
                self.sessions[s].request_close_gui = true;
            }
        }
    }

    /// Close player `s`'s open menu session in the app-required cleanup order:
    /// cursor stack, player-crafting output, furnace, chest, then any mod
    /// GUI (whose session state map is cleared with it).
    pub fn close_open_menu_for(&mut self, s: usize, events: &mut TickEvents) {
        // `container_closed` for whatever session was actually open. Emitted
        // (not dispatched) here: the handler runs at the tick's next drain
        // point, like every queued event.
        if let Some((kind, anchor)) = container_event_key(self.sessions[s].menu.target()) {
            self.bus.emit(PostEvent::ContainerClosed { kind, anchor });
        }
        self.release_chest_viewer(s, events);
        self.close_cursor_stack_for(s);
        self.close_crafting_for(s);
        self.sessions[s].menu.close_furnace();
        self.sessions[s].menu.close_chest();
        self.close_registered_gui_for(s);
        if self.sessions[s].menu.target().kind()
            == Some(petramond_world::gui_state::GuiKind::Creative)
        {
            self.sessions[s].menu.close_document_gui();
        }
        self.clear_menu_open_requests(s);
    }

    /// `container_opened` for the session that just began. The `open_*_for`
    /// methods are the single funnel every container screen opens through
    /// (whether from a block interact, a mod action, or the inventory key),
    /// so the event fires exactly once per session.
    fn emit_container_opened(&mut self, s: usize) {
        if let Some((kind, anchor)) = container_event_key(self.sessions[s].menu.target()) {
            self.bus.emit(PostEvent::ContainerOpened { kind, anchor });
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
    fn close_registered_gui_for(&mut self, s: usize) {
        if self.sessions[s]
            .menu
            .target()
            .kind()
            .is_some_and(|kind| kind.is_registered())
        {
            let sess = &mut self.sessions[s];
            petramond_world::gui_state::gui_state_clear(&mut sess.gui_state);
            sess.menu.close_document_gui();
            if !self.any_registered_gui_open() {
                self.clear_all_gui_states();
            }
        }
    }

    fn any_registered_gui_open(&self) -> bool {
        self.sessions.iter().any(|sess| {
            sess.menu
                .target()
                .kind()
                .is_some_and(|kind| kind.is_registered())
        })
    }

    fn clear_all_gui_states(&mut self) {
        for sess in &mut self.sessions {
            petramond_world::gui_state::gui_state_clear(&mut sess.gui_state);
            sess.last_sent_gui_state = None;
        }
    }

    /// Session `s`'s menu view as the wire message, with `gui_state` held
    /// `None` (the caller attaches the map only when its `Arc` changed).
    pub(super) fn build_menu_sync_base(&self, s: usize) -> MenuSyncMsg {
        let sess = &self.sessions[s];
        // EVERY container ships as the generic keyed slot list plus whatever
        // named gauge readings its block entity publishes. No engine content
        // identity remains here, so adding one is not a wire change.
        let target = match sess.menu.target() {
            ContainerTarget::None => MenuTargetWire::None,
            ContainerTarget::Gui { kind, anchor } => match kind {
                kind if CraftingStation::of_kind(kind).is_some() => MenuTargetWire::Crafting {
                    output: slot_wire(sess.menu.craft_output()),
                },
                kind => {
                    // A machine's readings ride the SAME generic state map a
                    // pack GUI uses, so no engine machine needs a wire variant.
                    let gauges = sess.menu.open_gauges(&self.world);
                    MenuTargetWire::Container {
                        kind_key: petramond_world::gui_state::kind_key(kind)
                            .unwrap_or_default()
                            .to_string(),
                        anchor,
                        slots: sess
                            .menu
                            .open_container_view(&self.world)
                            .map(|v| v.slots.iter().map(|s| slot_wire(*s)).collect()),
                        gui_state: (!gauges.is_empty()).then(|| {
                            gauges
                                .into_iter()
                                .map(|(k, v)| {
                                    (
                                        k,
                                        GuiValueWire::from_value(
                                            &petramond_world::gui_state::GuiValue::F32(v),
                                        ),
                                    )
                                })
                                .collect()
                        }),
                    }
                }
            },
        };
        MenuSyncMsg { target }
    }
}

/// The `container_opened`/`container_closed` payload for a menu target, or `None`
/// when no container session is involved. The unified target already carries
/// the event's `(kind, anchor)` identity.
fn container_event_key(
    target: ContainerTarget,
) -> Option<(petramond_world::gui_state::GuiKind, Option<MenuAnchor>)> {
    match target {
        ContainerTarget::None => None,
        ContainerTarget::Gui { kind, anchor } => Some((kind, anchor)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::player::PendingMenuAction;
    use petramond_world::gui_state::intern_kind;
    use petramond_world::item::ItemType;

    fn server_with_carrier(slots: Vec<Option<ItemStack>>) -> (ServerGame, u64) {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        server.world.mobs_mut().restore([crate::mob::SavedMob {
            kind: crate::mob::Mob::Owl,
            pos: petramond_math::world_pos::WorldPos::new(8.5, 64.0, 8.5),
            yaw: 0.0,
            tags: Default::default(),
            container: petramond_world::container::Container { slots },
        }]);
        let mob = server.world.mobs().instances()[0].id();
        (server, mob)
    }

    fn open_on_mob(server: &mut ServerGame, mob: u64, events: &mut TickEvents) {
        let kind = intern_kind("anchortest:pack").unwrap();
        server.sessions[0]
            .pending_menu_actions
            .push(PendingMenuAction::OpenGui {
                kind,
                anchor: Some(MenuAnchor::Mob(mob)),
            });
        server.tick_menu(0, events);
    }

    fn click(server: &mut ServerGame, slot: MenuSlot, events: &mut TickEvents) {
        server.sessions[0]
            .pending_menu_actions
            .push(PendingMenuAction::SlotClick {
                slot,
                button: PointerButton::Primary,
                shift: false,
                gather: false,
                request_id: 0,
            });
        server.tick_menu(0, events);
    }

    #[test]
    fn a_mob_anchored_session_moves_items_between_the_player_and_the_mob() {
        let (mut server, mob) =
            server_with_carrier(vec![None, Some(ItemStack::new(ItemType::Stone, 2))]);
        let mut events = TickEvents::default();
        let inv_slot = {
            let inv = &mut server.sessions[0].player.inventory;
            inv.add(ItemStack::new(ItemType::Coal, 5));
            (0..petramond_world::inventory::TOTAL_SLOTS)
                .find(|i| inv.slot(*i).is_some_and(|s| s.item == ItemType::Coal))
                .expect("the coal landed somewhere")
        };
        open_on_mob(&mut server, mob, &mut events);
        assert_eq!(
            server.sessions[0].menu.target().anchor(),
            Some(MenuAnchor::Mob(mob))
        );

        click(&mut server, MenuSlot::Inventory(inv_slot), &mut events);
        click(&mut server, MenuSlot::Container(0), &mut events);
        click(&mut server, MenuSlot::Container(1), &mut events);

        let carried = server.world.mobs().instances()[0].container();
        assert_eq!(carried.slots[0], Some(ItemStack::new(ItemType::Coal, 5)));
        assert_eq!(carried.slots[1], None);
        assert_eq!(
            server.sessions[0].player.inventory.cursor().copied(),
            Some(ItemStack::new(ItemType::Stone, 2))
        );
        let MenuTargetWire::Container { slots, .. } = server.build_menu_sync_base(0).target else {
            panic!("a mob-anchored session syncs as a container");
        };
        assert_eq!(slots.map(|s| s.len()), Some(2), "the mob's slots replicate");
    }

    #[test]
    fn a_mob_anchored_session_closes_when_the_mob_is_gone() {
        let (mut server, mob) = server_with_carrier(vec![None]);
        let mut events = TickEvents::default();
        open_on_mob(&mut server, mob, &mut events);
        server.close_menus_on_absent_anchors(&mut events);
        assert_ne!(server.sessions[0].menu.target(), ContainerTarget::None);

        server.world.mobs_mut().remove(0);
        server.close_menus_on_absent_anchors(&mut events);
        assert_eq!(server.sessions[0].menu.target(), ContainerTarget::None);
        assert!(server.sessions[0].request_close_gui);

        open_on_mob(&mut server, mob, &mut events);
        assert_eq!(
            server.sessions[0].menu.target(),
            ContainerTarget::None,
            "a session cannot open on a mob that is not there"
        );
    }
}
