use crate::events::PostEvent;
use crate::net::protocol::{GuiValueWire, ItemSlotWire, MenuSyncMsg, MenuTargetWire};
use petramond_math::math::IVec3;
use petramond_world::crafting::CraftingStation;
use petramond_world::gui_state::MenuSlot;
use petramond_world::gui_state::PointerButton;
use petramond_world::item::ItemStack;

use super::game::ServerGame;
use crate::events::tick::TickEvents;
use crate::menu::{ContainerTarget, CraftMenuFailure, MenuAnchor};
use crate::net::protocol::ActionDenyReason;
use crate::server::player::PendingMenuAction;

fn slot_wire(slot: Option<ItemStack>) -> Option<ItemSlotWire> {
    slot.map(ItemSlotWire::from_stack)
}

impl ServerGame {
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
        for action in self.sessions[s].input.take_menu_actions() {
            match action {
                PendingMenuAction::OpenGui { kind, anchor } => {
                    self.replace_open_menu_for(s, events);
                    if self.open_gui_for(s, kind, anchor, events) {
                        self.sessions[s].replication.request_open_gui = Some((kind, anchor));
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
                        && self.sessions[s].sim.menu.target().kind() == Some(GuiKind::Creative);
                    let resolved = item
                        .as_deref()
                        .and_then(ItemType::by_name)
                        .filter(|i| i.creative_visible());
                    let accepted = allowed && (item.is_none() || resolved.is_some());
                    if accepted {
                        *self.sessions[s].player.inventory.cursor_mut() =
                            resolved.map(|i| ItemStack::new(i, i.max_stack_size()));
                    }
                    self.sessions[s].replication.force_inventory_resync();
                    self.sessions[s].replication.force_menu_resync();
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
                        if button == PointerButton::Primary {
                            self.dispatch_gui_click(s, id, events);
                        }
                    } else {
                        let sess = &mut self.sessions[s];
                        let gui = sess.sim.gui_state.clone();
                        sess.sim.menu.click(
                            &mut self.world,
                            &mut sess.player.inventory,
                            Some(&gui),
                            slot,
                            button,
                            shift,
                            gather,
                        );
                        sess.replication.force_inventory_resync();
                        sess.replication.force_menu_resync();
                    }
                    self.push_action_outcome(s, request_id, true, None);
                }
                PendingMenuAction::SlotDrag {
                    slots,
                    button,
                    request_id,
                } => {
                    let sess = &mut self.sessions[s];
                    let gui = sess.sim.gui_state.clone();
                    sess.sim.menu.drag_slots(
                        &mut self.world,
                        &mut sess.player.inventory,
                        Some(&gui),
                        &slots,
                        button,
                    );
                    // Drag predicts on both mirrors, so force the resync even when stale client
                    // capacity made the server action a no-op and no on-change gate fired.
                    sess.replication.force_inventory_resync();
                    sess.replication.force_menu_resync();
                    self.push_action_outcome(s, request_id, true, None);
                }
                PendingMenuAction::DropSlot {
                    slot,
                    all,
                    request_id,
                } => {
                    let dropped = {
                        let sess = &mut self.sessions[s];
                        sess.sim.menu.drop_slot(
                            &mut self.world,
                            &mut sess.player.inventory,
                            slot,
                            all,
                        )
                    };
                    if let Some(stack) = dropped {
                        self.sessions[s].sim.drop_queue.queue_stack(stack);
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
                        let gui = sess.sim.gui_state.clone();
                        sess.sim.menu.swap_off_hand(
                            &mut self.world,
                            &mut sess.player.inventory,
                            Some(&gui),
                            slot,
                        );
                        sess.replication.force_inventory_resync();
                        sess.replication.force_menu_resync();
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
                        let (player, menu) = (&mut sess.player, &mut sess.sim.menu);
                        menu.craft_recipe(
                            &mut player.inventory,
                            self.catalog.recipes(),
                            &player.progression,
                            &recipe,
                            bulk,
                        )
                    };
                    match result {
                        Ok(overflow) => {
                            for stack in overflow {
                                self.sessions[s].sim.drop_queue.queue_stack(stack);
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

    fn replace_open_menu_for(&mut self, s: usize, events: &mut TickEvents) {
        if self.sessions[s].sim.menu.target() != ContainerTarget::None {
            self.close_open_menu_for(s, events);
        } else {
            self.clear_menu_open_requests(s);
        }
    }

    fn clear_menu_open_requests(&mut self, s: usize) {
        self.sessions[s].replication.request_open_gui = None;
    }

    fn open_gui_for(
        &mut self,
        s: usize,
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<MenuAnchor>,
        events: &mut TickEvents,
    ) -> bool {
        let pos = anchor.and_then(MenuAnchor::block);
        if let Some(station) = CraftingStation::of_kind(kind) {
            self.open_crafting_for(s, station);
            return true;
        }
        use petramond_world::gui_state::GuiKind;
        match kind {
            GuiKind::Creative if self.sessions[s].player.abilities().item_catalog => {
                self.sessions[s]
                    .sim
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

    fn dispatch_gui_click(
        &mut self,
        s: usize,
        widget_id: petramond_world::gui_state::WidgetId,
        events: &mut TickEvents,
    ) {
        let ContainerTarget::Gui { kind, anchor } = self.sessions[s].sim.menu.target() else {
            return;
        };
        if !kind.is_registered() || CraftingStation::of_kind(kind).is_some() {
            return;
        }
        let Some(kind_key) = petramond_world::gui_state::kind_key(kind) else {
            return;
        };
        let actor = Some(self.sessions[s].id);
        let Self {
            world,
            sessions,
            mods,
            ..
        } = self;
        mods.dispatch(world, sessions, actor, events, |host, ctx| {
            host.dispatch_gui_click(ctx, kind_key, widget_id, anchor)
        });
    }

    pub fn open_crafting_for(&mut self, s: usize, station: CraftingStation) {
        let sess = &mut self.sessions[s];
        sess.sim.menu.open_crafting(station);
        self.emit_container_opened(s);
    }

    pub fn open_furnace_screen_for(&mut self, s: usize, pos: IVec3) {
        let sess = &mut self.sessions[s];
        sess.sim.menu.open_furnace_screen(&mut self.world, pos);
        self.emit_container_opened(s);
    }

    pub fn open_chest_screen_for(&mut self, s: usize, pos: IVec3, events: &mut TickEvents) {
        let same = matches!(
            self.sessions[s].sim.menu.target(),
            ContainerTarget::Gui { kind: petramond_world::gui_state::GuiKind::Chest, anchor: Some(MenuAnchor::Block(p)) } if p == pos
        );
        if !same {
            self.release_chest_viewer(s, events);
        }
        let sess = &mut self.sessions[s];
        sess.sim.menu.open_chest_screen(&mut self.world, pos);
        if !same {
            self.containers.add_viewer(pos, events);
        }
        self.emit_container_opened(s);
    }

    fn release_chest_viewer(&mut self, s: usize, events: &mut TickEvents) {
        if let ContainerTarget::Gui {
            kind: petramond_world::gui_state::GuiKind::Chest,
            anchor: Some(MenuAnchor::Block(pos)),
        } = self.sessions[s].sim.menu.target()
        {
            self.containers.drop_viewer(pos, events);
        }
    }

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
        petramond_world::gui_state::gui_state_clear(&mut sess.sim.gui_state);
        sess.sim
            .menu
            .open_document_gui(&mut self.world, kind, anchor);
        self.emit_container_opened(s);
    }

    pub(super) fn close_menus_on_absent_anchors(&mut self, events: &mut TickEvents) {
        for s in 0..self.sessions.len() {
            let gone = self.sessions[s]
                .sim
                .menu
                .target()
                .anchor()
                .is_some_and(|anchor| !anchor.present(&self.world));
            if gone {
                self.close_open_menu_for(s, events);
                self.sessions[s].replication.request_close_gui = true;
            }
        }
    }

    pub fn close_open_menu_for(&mut self, s: usize, events: &mut TickEvents) {
        if let Some((kind, anchor)) = container_event_key(self.sessions[s].sim.menu.target()) {
            self.mods.emit(PostEvent::ContainerClosed {
                player: self.sessions[s].id,
                kind,
                anchor,
            });
        }
        self.release_chest_viewer(s, events);
        self.close_cursor_stack_for(s);
        self.close_crafting_for(s);
        self.sessions[s].sim.menu.close_furnace();
        self.sessions[s].sim.menu.close_chest();
        self.close_registered_gui_for(s);
        if self.sessions[s].sim.menu.target().kind()
            == Some(petramond_world::gui_state::GuiKind::Creative)
        {
            self.sessions[s].sim.menu.close_document_gui();
        }
        self.clear_menu_open_requests(s);
    }

    fn emit_container_opened(&mut self, s: usize) {
        if let Some((kind, anchor)) = container_event_key(self.sessions[s].sim.menu.target()) {
            self.mods.emit(PostEvent::ContainerOpened {
                player: self.sessions[s].id,
                kind,
                anchor,
            });
        }
    }

    fn close_crafting_for(&mut self, s: usize) {
        let mut overflow = Vec::new();
        let sess = &mut self.sessions[s];
        sess.sim
            .menu
            .close_crafting(&mut sess.player.inventory, |stack| overflow.push(stack));
        for stack in overflow {
            sess.sim.drop_queue.queue_stack(stack);
        }
    }

    fn close_registered_gui_for(&mut self, s: usize) {
        if self.sessions[s]
            .sim
            .menu
            .target()
            .kind()
            .is_some_and(|kind| kind.is_registered())
        {
            let sess = &mut self.sessions[s];
            petramond_world::gui_state::gui_state_clear(&mut sess.sim.gui_state);
            sess.sim.menu.close_document_gui();
            if !self.any_registered_gui_open() {
                self.clear_all_gui_states();
            }
        }
    }

    fn any_registered_gui_open(&self) -> bool {
        self.sessions.iter().any(|sess| {
            sess.sim
                .menu
                .target()
                .kind()
                .is_some_and(|kind| kind.is_registered())
        })
    }

    fn clear_all_gui_states(&mut self) {
        for sess in &mut self.sessions {
            petramond_world::gui_state::gui_state_clear(&mut sess.sim.gui_state);
            sess.replication.force_gui_state_resync();
        }
    }

    pub(super) fn build_menu_sync_base(&self, s: usize) -> MenuSyncMsg {
        let sess = &self.sessions[s];
        let target = match sess.sim.menu.target() {
            ContainerTarget::None => MenuTargetWire::None,
            ContainerTarget::Gui { kind, anchor } => match kind {
                kind if CraftingStation::of_kind(kind).is_some() => MenuTargetWire::Crafting {
                    output: slot_wire(sess.sim.menu.craft_output()),
                },
                kind => {
                    let gauges = sess.sim.menu.open_gauges(&self.world);
                    MenuTargetWire::Container {
                        kind_key: petramond_world::gui_state::kind_key(kind)
                            .unwrap_or_default()
                            .to_string(),
                        anchor,
                        slots: sess
                            .sim
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
        server.queue_menu_action(
            0,
            PendingMenuAction::OpenGui {
                kind,
                anchor: Some(MenuAnchor::Mob(mob)),
            },
        );
        server.tick_menu(0, events);
    }

    fn click(server: &mut ServerGame, slot: MenuSlot, events: &mut TickEvents) {
        server.queue_menu_action(
            0,
            PendingMenuAction::SlotClick {
                slot,
                button: PointerButton::Primary,
                shift: false,
                gather: false,
                request_id: 0,
            },
        );
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
            server.sessions[0].sim.menu.target().anchor(),
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
        assert_ne!(server.sessions[0].sim.menu.target(), ContainerTarget::None);

        server.world.mobs_mut().remove(mob);
        server.close_menus_on_absent_anchors(&mut events);
        assert_eq!(server.sessions[0].sim.menu.target(), ContainerTarget::None);
        assert!(server.sessions[0].replication.request_close_gui);

        open_on_mob(&mut server, mob, &mut events);
        assert_eq!(
            server.sessions[0].sim.menu.target(),
            ContainerTarget::None,
            "a session cannot open on a mob that is not there"
        );
    }
}
