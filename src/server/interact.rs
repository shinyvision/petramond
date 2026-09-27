use super::game::ServerGame;
use crate::events::tick::TickEvents;
use crate::events::{InteractAttempt, Outcome, PostEvent};
use crate::net::protocol::TargetRef;
use crate::player::UseGesture;
use crate::rules::interact::{ConsumerKind, USE_REPEAT_TICKS};
use crate::rules::use_click::{self, UseClickConsumers};
use crate::server::player::PendingUseClick;
use petramond_math::math::IVec3;
use petramond_world::block::{Block, BlockInteraction};

pub struct ClickMeta {
    pub target: Option<TargetRef>,
    pub predicted: bool,
    pub repeat: bool,
}

type Claim = use_click::Claim<IVec3>;

type Consume = fn(&mut ServerGame, usize, &InteractAttempt, &ClickMeta, &mut TickEvents) -> Claim;

fn consume_fn(kind: ConsumerKind) -> Consume {
    match kind {
        ConsumerKind::Registered => ServerGame::consume_registered_attempt,
        ConsumerKind::Shear => ServerGame::consume_shear,
        ConsumerKind::BuiltinBlock => ServerGame::consume_builtin_block,
        ConsumerKind::ContextualPlace => ServerGame::consume_contextual_place,
        ConsumerKind::Eat => ServerGame::consume_eat,
        ConsumerKind::ItemUse => ServerGame::consume_item_use,
        ConsumerKind::Place => ServerGame::consume_place,
    }
}

struct ServerConsumers<'a> {
    game: &'a mut ServerGame,
    s: usize,
    attempt: &'a InteractAttempt,
    meta: &'a ClickMeta,
    events: &'a mut TickEvents,
}

impl UseClickConsumers for ServerConsumers<'_> {
    type Placement = IVec3;

    fn off_hand_occupied(&self) -> bool {
        self.game.sessions[self.s]
            .player
            .inventory
            .off_hand()
            .is_some()
    }

    fn set_acting_hand(&mut self, hand: petramond_world::inventory::Hand) {
        self.game.sessions[self.s].player.acting_hand = hand;
    }

    fn offer(&mut self, kind: ConsumerKind) -> Claim {
        consume_fn(kind)(self.game, self.s, self.attempt, self.meta, self.events)
    }
}

impl ServerGame {
    pub fn tick_place(&mut self, s: usize, events: &mut TickEvents) {
        if !self.sessions[s].using() {
            self.sessions[s].player.use_gesture = UseGesture::Free;
        }
        if self.sessions[s]
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Use)
        {
            self.sessions[s].input.pending_use_click = None;
            self.advance_eating(s);
            return;
        }
        if !self.sessions[s].player.use_gesture.is_free() {
            self.sessions[s].input.pending_use_click = None;
            self.advance_eating(s);
            return;
        }
        if let Some(click) = self.sessions[s].input.pending_use_click.take() {
            if click.selection_still_matches(&self.sessions[s].player) {
                self.dispatch_use_click(s, click, events, false);
            } else {
                self.reject_selection_changed_use_click(s, click);
            }
            self.sessions[s].input.use_repeat_cooldown = USE_REPEAT_TICKS;
        } else {
            self.tick_use_repeat(s, events);
        }
        self.advance_eating(s);
    }

    fn tick_use_repeat(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        if !sess.input.intent_use_held || sess.sim.eating.is_some() {
            return;
        }
        sess.input.use_repeat_cooldown = sess.input.use_repeat_cooldown.saturating_sub(1);
        if sess.input.use_repeat_cooldown > 0 {
            return;
        }
        sess.input.use_repeat_cooldown = USE_REPEAT_TICKS;
        let look = sess.input.look;
        let click = PendingUseClick::capture(&sess.player, None, look, None, false, false);
        self.dispatch_use_click(s, click, events, true);
    }

    fn dispatch_use_click(
        &mut self,
        s: usize,
        click: PendingUseClick,
        events: &mut TickEvents,
        repeat: bool,
    ) {
        let PendingUseClick {
            mob,
            target,
            request_id,
            predicted,
            jabbed,
            ..
        } = click;
        let mob = super::mob_target::authoritative_mob_target(&self.world, &self.sessions[s], mob);
        let attempt = InteractAttempt {
            block: target.map(|t| t.block),
            face: target.map(|t| t.normal),
            mob,
            player: self.sessions[s].id,
        };
        let meta = ClickMeta {
            target,
            predicted,
            repeat,
        };
        let outcome = use_click::run_use_click(&mut ServerConsumers {
            game: &mut *self,
            s,
            attempt: &attempt,
            meta: &meta,
            events: &mut *events,
        });
        let consumed = outcome.consumed();
        let off_hand_acted = outcome.off_hand_acted();
        let placed_at = outcome.placement;
        events.player(s).click_off_hand = off_hand_acted;
        if consumed {
            let hand = if off_hand_acted {
                petramond_world::inventory::Hand::Off
            } else {
                petramond_world::inventory::Hand::Main
            };
            self.sessions[s].latch_swing(hand, mod_api::SwingKind::Interact);
        }
        // NOTHING took it. This is the fall-through a CONTINUOUS use lives on
        // (`HoldUse`): a shield raises here and nowhere else, which is what
        // stops it going up on the click that opened a door. Fired even with
        // no target at all — a click at empty air is the ordinary way to raise
        // one.
        // ...and only on a REAL press. A continuous use starts from a deliberate
        // right click, never from the server-paced repeat — the same rule the
        // engine's own eat follows (`consume_eat` refuses on a repeat).
        //
        // It is also what keeps the two mirrors honest: the client predicts the
        // PRESS and nothing else, so a gesture taken on a repeat would be one
        // the client never saw. The body would slow and its hands go quiet
        // while the shield stayed visually down — the authority guarding and
        // the picture disagreeing, for as long as the button was held.
        if !consumed && !meta.repeat {
            let mut ev = attempt;
            let claimed = {
                let Self {
                    world,
                    sessions,
                    mods,
                    ..
                } = self;
                let actor = Some(sessions[s].id);
                let bus = mods.bus_mut();
                bus.use_unclaimed(world, sessions, actor, events, &mut ev) == Outcome::Cancel
            };
            let _ = claimed;
        }
        if attempt.block.is_some() || attempt.mob.is_some() {
            self.mods.emit(PostEvent::Interacted {
                block: attempt.block,
                face: attempt.face,
                mob: attempt.mob,
                player: attempt.player,
                consumed,
            });
        }
        if consumed && !jabbed && !outcome.presents_itself() {
            events.player(s).used_unpredicted = true;
        }
        // The client's ghost convention is `target.block + normal` — accept
        // ONLY a placement that landed exactly there (accept never rolls
        // back, so anything else must DENY to clear the ghost: a click
        // claimed by an interact/eat/use, a replace-in-place, a slab stack
        // into the hit cell, nothing at all).
        let predicted_cell = target
            .filter(|t| t.normal != IVec3::ZERO)
            .map(|t| t.block + t.normal);
        let accepted = placed_at.is_some() && placed_at == predicted_cell;
        if let Some(id) = request_id {
            self.push_action_outcome(
                s,
                id,
                accepted,
                (!accepted).then_some(crate::net::protocol::ActionDenyReason::Denied),
            );
        }
        if let Some(t) = target.filter(|_| !repeat) {
            let disputed = !consumed || (request_id.is_some() && !accepted);
            if disputed {
                self.queue_use_corrective_cells(s, t);
            }
        }
    }

    fn reject_selection_changed_use_click(&mut self, s: usize, click: PendingUseClick) {
        if let Some(id) = click.request_id {
            self.push_action_outcome(
                s,
                id,
                false,
                Some(crate::net::protocol::ActionDenyReason::Denied),
            );
        }
        if let Some(target) = click.target {
            self.queue_use_corrective_cells(s, target);
        }
    }

    fn queue_use_corrective_cells(&mut self, s: usize, target: TargetRef) {
        let sess = &mut self.sessions[s];
        sess.replication.pending_corrective_cells.push(target.block);
        if target.normal != IVec3::ZERO {
            sess.replication
                .pending_corrective_cells
                .push(target.block + target.normal);
        }
    }

    fn consume_registered_attempt(
        &mut self,
        s: usize,
        attempt: &InteractAttempt,
        _meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if !use_click::registered_offered(attempt.block, attempt.mob) {
            return Claim::Pass;
        }
        let mut ev = *attempt;
        let claimed = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let actor = Some(sessions[s].id);
            let bus = mods.bus_mut();
            bus.interact_attempt(world, sessions, actor, events, &mut ev) == Outcome::Cancel
        };
        if claimed {
            events.player(s).interacted = true;
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    fn consume_shear(
        &mut self,
        s: usize,
        attempt: &InteractAttempt,
        _meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if attempt.mob.is_none() {
            return Claim::Pass;
        }
        if self.try_shear_mob(s, attempt.mob) {
            events.player(s).used_item = true;
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    pub(super) fn swing_door(&mut self, pos: IVec3, events: &mut TickEvents) -> Option<bool> {
        let lower = self.world.door_lower_cell(pos.x, pos.y, pos.z)?;
        self.world.toggle_door(pos);
        let now_open = self
            .world
            .door_state_at(lower.x, lower.y, lower.z)
            .map(|st| st.open)
            .unwrap_or(true);
        events.world.panel_changed.push((lower, now_open));
        Some(now_open)
    }

    pub(super) fn swing_trapdoor(&mut self, pos: IVec3, events: &mut TickEvents) -> Option<bool> {
        let now_open = self.world.toggle_trapdoor(pos)?;
        events.world.panel_changed.push((pos, now_open));
        Some(now_open)
    }

    fn consume_builtin_block(
        &mut self,
        s: usize,
        attempt: &InteractAttempt,
        _meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        let Some(pos) = attempt.block else {
            return Claim::Pass;
        };
        if !use_click::builtin_claims_at(self.world.data(), pos, self.sessions[s].sneaking()) {
            return Claim::Pass;
        }
        let block = Block::from_id(self.world.data().chunk_block(pos.x, pos.y, pos.z));
        let claimed = match block.interaction() {
            BlockInteraction::OpenGui(kind) => {
                self.queue_menu_action(
                    s,
                    crate::server::player::PendingMenuAction::OpenGui {
                        kind,
                        anchor: Some(crate::menu::MenuAnchor::Block(pos)),
                    },
                );
                true
            }
            BlockInteraction::ToggleDoor => {
                let Some(now_open) = self.swing_door(pos, events) else {
                    return Claim::Pass;
                };
                events.player(s).toggled_panel = Some(now_open);
                true
            }
            BlockInteraction::ToggleTrapdoor => {
                let Some(now_open) = self.swing_trapdoor(pos, events) else {
                    return Claim::Pass;
                };
                events.player(s).toggled_panel = Some(now_open);
                true
            }

            BlockInteraction::Sleep => {
                let acted = self.start_sleep(s, pos);
                if acted {
                    events.player(s).bed_interacted = true;
                }
                acted
            }
            BlockInteraction::None => false,
        };
        if claimed {
            events.player(s).interacted = true;
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    fn held_is_contextual_placeable(&self, s: usize) -> bool {
        crate::rules::item_use::held_item(&self.sessions[s].player)
            .is_some_and(crate::rules::item_use::is_contextual_placeable)
    }

    fn consume_contextual_place(
        &mut self,
        s: usize,
        _attempt: &InteractAttempt,
        meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if !self.held_is_contextual_placeable(s) {
            return Claim::Pass;
        }
        match self.place_held(s, meta.target, meta.predicted, events) {
            Some(pos) => Claim::Placed(pos),
            None => Claim::Pass,
        }
    }

    fn consume_eat(
        &mut self,
        s: usize,
        _attempt: &InteractAttempt,
        meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if !meta.repeat && self.try_start_eating(s, events) {
            self.sessions[s].player.use_gesture =
                UseGesture::Held(crate::player::ENGINE_CLAIMANT.into());
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    fn consume_item_use(
        &mut self,
        s: usize,
        _attempt: &InteractAttempt,
        meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if self.try_use_item(s, meta.target, events) {
            events.player(s).used_item = true;
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }

    fn consume_place(
        &mut self,
        s: usize,
        _attempt: &InteractAttempt,
        meta: &ClickMeta,
        events: &mut TickEvents,
    ) -> Claim {
        if self.held_is_contextual_placeable(s) {
            return Claim::Pass;
        }
        match self.place_held(s, meta.target, meta.predicted, events) {
            Some(pos) => Claim::Placed(pos),
            None => Claim::Pass,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn queue_place_click_for_test(&mut self, s: usize) {
        let sess = &mut self.sessions[s];
        sess.input.intent_gameplay = true;
        sess.input.pending_use_click = Some(PendingUseClick::capture(
            &sess.player,
            None,
            sess.input.look,
            None,
            true,
            false,
        ));
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn queue_mob_use_click_for_test(&mut self, s: usize, mob: u64) {
        let sess = &mut self.sessions[s];
        sess.input.intent_gameplay = true;
        sess.input.pending_use_click = Some(PendingUseClick::capture(
            &sess.player,
            Some(mob),
            None,
            None,
            true,
            false,
        ));
    }
}

#[cfg(test)]
mod tests {
    use crate::net::protocol::{PlayerAction, TargetRef};
    use petramond_math::math::{IVec3, Vec3};
    use petramond_math::world_pos::WorldPos;
    use petramond_world::block::Block;
    use petramond_world::item::{ItemStack, ItemType};

    #[test]
    fn boat_use_click_on_water_spawns_the_hull() {
        let Some(root) = crate::modding::tests::stage_mods_fixture(
            "boat-place",
            &[
                "exploration",
                "farming",
                "forge",
                "furniture",
                "kitchen",
                "monsters",
                "vehicles",
                "weather",
            ],
        ) else {
            return;
        };
        crate::modding::tests::run_child_test(
            &root,
            "server::interact::tests::boat_use_click_on_water_spawns_the_hull_inner",
        );
    }

    const STAGE_REACH: i32 = 12;
    const STAGE_HEADROOM: i32 = 4;

    #[test]
    #[ignore = "spawned by boat_use_click_on_water_spawns_the_hull with the vehicles pack env"]
    fn boat_use_click_on_water_spawns_the_hull_inner() {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        for _ in 0..40 {
            server.pump_tagged(0.06, &mut Vec::new(), &[]);
        }

        let feet = server.sessions[0].player.pos;
        let (bx, by, bz) = (
            feet.x.floor() as i32,
            feet.y.floor() as i32,
            feet.z.floor() as i32,
        );
        for dx in -STAGE_REACH..=STAGE_REACH {
            for dz in -STAGE_REACH..=STAGE_REACH {
                server
                    .world
                    .set_block_world(bx + dx, by - 1, bz + dz, Block::Stone);
                for dy in 0..=STAGE_HEADROOM {
                    server
                        .world
                        .set_block_world(bx + dx, by + dy, bz + dz, Block::Air);
                }
            }
        }
        {
            let sess = &mut server.sessions[0];
            let standing = WorldPos::new(bx as f64 + 0.5, by as f64, bz as f64 + 0.5);
            sess.player.pos = standing;
            sess.player.vel = Vec3::ZERO;
            sess.input.claim_pos = standing;
        }

        let (wx, wy, wz) = (bx + 5, by, bz);
        for dx in -4..=4 {
            for dz in -4..=4 {
                let edge = dx == -4 || dx == 4 || dz == -4 || dz == 4;
                server.world.set_block_world(
                    wx + dx,
                    wy,
                    wz + dz,
                    if edge { Block::Stone } else { Block::Water },
                );
            }
        }

        server.sessions[0].input.intent_gameplay = true;

        let boat = ItemType::by_key("vehicles:boat").expect("the vehicles pack item registered");
        server.sessions[0]
            .player
            .inventory
            .add(ItemStack::new(boat, 1));
        assert_eq!(
            server.sessions[0]
                .player
                .inventory
                .selected()
                .map(|st| st.item),
            Some(boat),
            "the boat sits in the selected hotbar slot"
        );

        {
            let sess = &mut server.sessions[0];
            let eye = crate::server::movement::reach_eye(sess);
            let dir = (petramond_math::world_pos::WorldPos::new(
                wx as f64 - 3.0 + 0.5,
                wy as f64 + 0.95,
                wz as f64 + 0.5,
            ) - eye)
                .normalize();
            sess.player.pitch = dir.y.asin();
            sess.player.yaw = dir.x.atan2(dir.z);
        }
        let (eye, fwd) = {
            let sess = &server.sessions[0];
            (
                crate::server::movement::reach_eye(sess),
                sess.player.forward(),
            )
        };
        let ray = petramond_world::item::UseRay::Fluids(&[Block::Water]);
        let (hit, _) = petramond_world::world::raycast::use_ray(eye, fwd, server.world.data(), ray)
            .expect("the aim ray reaches the pool");
        assert_eq!(
            hit.block,
            IVec3::new(wx - 3, wy, wz),
            "test geometry: the shore-hugging water cell is the first hit"
        );

        server.apply_action(
            0,
            PlayerAction::UseClick {
                mob: None,
                target: Some(TargetRef::face(hit.block, hit.normal)),
                request_id: None,
                predicted: false,
                jabbed: false,
            },
        );
        assert!(
            server.sessions[0]
                .input
                .pending_use_click
                .is_some_and(|c| c.target.is_some()),
            "the receipt-time water-ray validator accepted the claimed cell"
        );
        server.pump_tagged(0.06, &mut Vec::new(), &[]);

        let kind = crate::mob::by_key("vehicles:boat").expect("the boat species registered");
        let hulls: Vec<petramond_math::world_pos::WorldPos> = server
            .world
            .mobs()
            .instances()
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| m.pos)
            .collect();
        assert_eq!(hulls.len(), 1, "the click launched exactly one hull");
        assert!(
            hulls[0].x > wx as f64 - 2.5,
            "the hull was nudged toward open water, off the shore-hugging click"
        );
        assert_ne!(
            server.sessions[0]
                .player
                .inventory
                .selected()
                .map(|st| st.item),
            Some(boat),
            "the placed boat item was spent"
        );
    }
}
