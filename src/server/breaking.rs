use crate::entity::DroppedItem;
use crate::events::{BlockBreakPre, Outcome, PostEvent};
use crate::rules::breaking::break_light;
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::item::ItemStack;
use petramond_world::mining::{BreakEvent, MiningState};

use super::game::ServerGame;
use crate::events::tick::{BlockBrokenEvent, TickEvents, TICK_DT};

#[derive(Clone, Copy, Debug)]
pub(super) struct Breaker {
    pub actor: crate::mob::EntityRef,
    pub yields: bool,
    pub collector: Option<u64>,
    pub normal: Option<IVec3>,
}

const BREAK_ACK_TTL_TICKS: u64 = 200;

const INSTANT_BREAK_REPEAT_TICKS: u64 = 3;

impl ServerGame {
    pub fn tick_mining(&mut self, s: usize, events: &mut TickEvents) {
        let now = self.world.current_tick();
        self.sessions[s]
            .input
            .pending_break_ack
            .retain(|_, broke_at| now.saturating_sub(*broke_at) <= BREAK_ACK_TTL_TICKS);
        self.close_open_edit(s, events);
        for req in self.sessions[s].input.take_break_finished() {
            self.resolve_break_finished(s, req, events);
        }

        if self.sessions[s].player.abilities().instant_break {
            self.sessions[s].sim.mining = MiningState::new();
            return;
        }
        let tool = self.sessions[s]
            .player
            .inventory
            .selected()
            .and_then(|st| st.tool());
        let look = self.sessions[s].input.look;
        let break_held = self.sessions[s].input.intent_break_held;
        // A barred mine reads exactly like a released button: the timer RESETS
        // rather than pausing, so releasing the claim starts the break over
        // instead of resuming a cell the player stopped looking at. The open
        // menu is one of the claims.
        let barred = self.sessions[s]
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Mine);
        if let Some(event) = self.sessions[s].sim.mining.update(
            TICK_DT,
            look.map(|t| t.block),
            break_held,
            barred,
            self.world.data(),
            tool,
        ) {
            // Hold-path finish: accept a deferred TooFast BreakFinished for this cell here.
            // Only strip the initiator's BlockBroken on evidence they presented locally
            // (deferred finish flagged `predicted`). Without a latched request the client may
            // never present: its per-frame timer can lag behind a tick-sampled look after a
            // sub-tick crosshair flicker, so the break delta cancels its mining first.
            // An in-flight finish still presents exactly once, since the
            // client's own presented cell suppresses the wire event until the request
            // resolves (see `predicted_presentation_cells` in `game/replicated.rs`).
            let broken_pos = event.pos;
            let deferred = self.sessions[s]
                .input
                .deferred_break_finished
                .take_if(|d| d.pos == broken_pos);
            let presented = deferred.is_some_and(|d| d.predicted);
            if self.finish_player_break(s, event, events, presented) {
                if let Some(req) = deferred {
                    self.sessions[s].input.pending_break_ack.remove(&broken_pos);
                    self.push_action_outcome(s, req.request_id, true, None);
                }
            } else if let Some(req) = deferred {
                self.deny_break_finished(
                    s,
                    req.request_id,
                    req.pos,
                    crate::net::protocol::ActionDenyReason::Denied,
                );
            }
        } else {
            self.abandon_deferred_break_if_stale(s);
        }
    }

    fn abandon_deferred_break_if_stale(&mut self, s: usize) {
        let Some(req) = self.sessions[s].input.deferred_break_finished else {
            return;
        };
        let still_mining = self.sessions[s]
            .sim
            .mining
            .progress()
            .is_some_and(|(target, _)| target == req.pos);
        if still_mining {
            return;
        }
        let req = self.sessions[s]
            .input
            .deferred_break_finished
            .take()
            .unwrap();
        self.deny_break_finished(
            s,
            req.request_id,
            req.pos,
            crate::net::protocol::ActionDenyReason::TooFast,
        );
    }

    fn resolve_break_finished(
        &mut self,
        s: usize,
        req: crate::server::player::PendingBreakFinished,
        events: &mut TickEvents,
    ) {
        use crate::net::protocol::ActionDenyReason;
        let crate::server::player::PendingBreakFinished {
            request_id,
            pos,
            tool_item_id,
            predicted,
        } = req;

        if self.sessions[s]
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Mine)
        {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::Denied);
            return;
        }

        let eye = crate::server::movement::reach_eye(&self.sessions[s]);
        if !crate::player::block_within_reach(eye, pos) {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::OutOfReach);
            return;
        }

        let block = Block::from_id(self.world.data().chunk_block(pos.x, pos.y, pos.z));
        // Hold-path may have already cleared the cell before a lagged
        // BreakFinished arrives. If THIS session broke it, accept — never
        // deny/restore (that re-spawns the block and invites a second break).
        if block == Block::Air {
            if self.sessions[s]
                .input
                .pending_break_ack
                .remove(&pos)
                .is_some()
            {
                if let Some(old) = self.sessions[s].input.deferred_break_finished.take() {
                    if old.pos != pos {
                        self.deny_break_finished(
                            s,
                            old.request_id,
                            old.pos,
                            ActionDenyReason::TooFast,
                        );
                    } else {
                        self.push_action_outcome(
                            s,
                            old.request_id,
                            false,
                            Some(ActionDenyReason::TooFast),
                        );
                    }
                }
                self.push_action_outcome(s, request_id, true, None);
                return;
            }
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::Denied);
            return;
        }
        if self.sessions[s].player.abilities().instant_break {
            let now = self.world.current_tick();
            if self.sessions[s]
                .sim
                .last_instant_break
                .is_some_and(|last| now.saturating_sub(last) < INSTANT_BREAK_REPEAT_TICKS)
            {
                self.deny_break_finished(s, request_id, pos, ActionDenyReason::TooFast);
                return;
            }
            if self
                .world
                .break_footprint_cells(pos)
                .iter()
                .any(|p| !self.world.physics_cell_final_at(p.x, p.y, p.z))
            {
                self.deny_break_finished(s, request_id, pos, ActionDenyReason::Denied);
                return;
            }
            let event = BreakEvent {
                pos,
                block,
                harvested: false,
            };
            if self.finish_player_break(s, event, events, predicted) {
                self.sessions[s].sim.last_instant_break = Some(now);
                self.sessions[s].input.pending_break_ack.remove(&pos);
                self.push_action_outcome(s, request_id, true, None);
            } else {
                self.deny_break_finished(s, request_id, pos, ActionDenyReason::Denied);
            }
            return;
        }
        if block.hardness() < 0.0 {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::Denied);
            return;
        }

        let auth_row_tool = self.sessions[s]
            .player
            .inventory
            .selected()
            .and_then(|st| st.item.tool());
        let auth_tool = self.sessions[s]
            .player
            .inventory
            .selected()
            .and_then(|st| st.tool());
        let claimed_tool = tool_item_id
            .map(petramond_world::item::ItemType)
            .and_then(|it| it.tool());
        if claimed_tool != auth_row_tool {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::BadTool);
            return;
        }

        // Duration is validated against the SERVER'S OWN mining timer — the
        // hold-path `sess.sim.mining` that accrues from the latched look +
        // break_held every tick. Client-reported time is never trusted.
        // Instant blocks (expected 0) need no observed window.
        //
        // TooFast is DEFERRED, not denied: the client's optimistic clear
        // stays, and when the hold-path timer finishes the same cell we
        // accept + strip presentation. Immediate deny would restore the
        // block then re-break it (double sound/burst on slow links).
        let expected = petramond_world::mining::break_time(block, auth_tool);
        if expected > 0.0 {
            let observed = self.sessions[s]
                .sim
                .mining
                .progress()
                .and_then(|(target, elapsed)| (target == pos).then_some(elapsed));
            if observed.is_none_or(|elapsed| elapsed + 3.0 * TICK_DT < expected) {
                if let Some(old) = self.sessions[s].input.deferred_break_finished.take() {
                    self.deny_break_finished(s, old.request_id, old.pos, ActionDenyReason::TooFast);
                }
                self.sessions[s].input.deferred_break_finished =
                    Some(crate::server::player::PendingBreakFinished {
                        request_id,
                        pos,
                        tool_item_id,
                        predicted,
                    });
                return;
            }
        }

        let event = BreakEvent {
            pos,
            block,
            harvested: petramond_world::mining::harvests(block, auth_tool),
        };
        self.sessions[s].sim.mining = MiningState::new();
        if let Some(old) = self.sessions[s].input.deferred_break_finished.take() {
            if old.pos != pos {
                self.deny_break_finished(s, old.request_id, old.pos, ActionDenyReason::TooFast);
            } else {
                self.push_action_outcome(s, old.request_id, false, Some(ActionDenyReason::TooFast));
            }
        }
        if self.finish_player_break(s, event, events, predicted) {
            self.sessions[s].input.pending_break_ack.remove(&pos);
            self.push_action_outcome(s, request_id, true, None);
        } else {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::Denied);
        }
    }

    fn deny_break_finished(
        &mut self,
        s: usize,
        request_id: crate::net::protocol::ClientRequestId,
        pos: IVec3,
        reason: crate::net::protocol::ActionDenyReason,
    ) {
        self.push_action_outcome(s, request_id, false, Some(reason));
        self.queue_break_corrective_cells(s, pos);
    }

    fn queue_break_corrective_cells(&mut self, s: usize, pos: IVec3) {
        let cells = self.world.break_footprint_cells(pos);
        self.sessions[s]
            .replication
            .pending_corrective_cells
            .extend(cells);
    }

    pub fn finish_player_break(
        &mut self,
        s: usize,
        event: BreakEvent,
        events: &mut TickEvents,
        initiator_presented: bool,
    ) -> bool {
        let hit_normal = self.sessions[s]
            .input
            .look
            .filter(|h| h.block == event.pos && h.normal != IVec3::ZERO)
            .map(|h| h.normal);
        self.touch_edit_cells(s, self.world.break_footprint_cells(event.pos));
        let breaker = Breaker {
            actor: crate::mob::EntityRef::Player(self.sessions[s].id),
            yields: self.sessions[s].player.abilities().yields_drops,
            collector: None,
            normal: hit_normal,
        };
        if !self.break_block(breaker, event, events) {
            return false;
        }
        events.player(s).broke_block = Some(event.block);
        self.sessions[s].latch_swing(
            petramond_world::inventory::Hand::Main,
            mod_api::SwingKind::Break,
        );
        if initiator_presented {
            self.sessions[s]
                .replication
                .presented_breaks
                .push(event.pos);
        }
        let now = self.world.current_tick();
        self.sessions[s]
            .input
            .pending_break_ack
            .insert(event.pos, now);
        true
    }

    pub(super) fn break_block(
        &mut self,
        breaker: Breaker,
        event: BreakEvent,
        events: &mut TickEvents,
    ) -> bool {
        let drops_override = {
            let mut pre = BlockBreakPre {
                pos: event.pos,
                block: event.block,
                harvested: event.harvested,
                actor: breaker.actor,
                drops: None,
            };
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let cancelled = mods.bus_mut().block_break_pre(
                world,
                sessions,
                breaker.actor.player(),
                events,
                &mut pre,
            ) == Outcome::Cancel;
            if cancelled {
                return false;
            }
            pre.drops
        };
        if event.block.has_tag(petramond_world::block::BlockTag::BED) {
            self.clear_bed_spawn_at(event.pos);
        }
        let (sky, blk) = break_light(self.world.data(), event.pos, breaker.normal);
        // A COMPOSED cell (a slab stack) drops each of its parts as its own
        // material, each stamped with that part's own carried data, so a white
        // slab under an orange one comes back as one white and one orange —
        // resolved BEFORE the removal below wipes the state and KV it reads.
        // The engine asks the family; it does not know a slab from a stair.
        let part_drops = self
            .world
            .data()
            .cell_parts(event.pos)
            .map(|parts| self.part_drop_stacks(event.pos, &parts));
        let container_pos = self.world.container_anchor(event.pos);
        self.world.stock_loot(container_pos);
        let carry_variant = self.carry_variant_at(container_pos, event.block, 0);
        let broken_tint = self.world.data().cell_burst_tint(event.pos);
        if self.world.remove_compound(event.pos).is_none() {
            let below = Block::from_id(self.world.data().chunk_block(
                event.pos.x,
                event.pos.y - 1,
                event.pos.z,
            ));
            self.world.set_block_world(
                event.pos.x,
                event.pos.y,
                event.pos.z,
                event.block.break_residue(below),
            );
        }
        self.world.forget_block_entity_records(container_pos);
        if let Some(container) = self.world.take_container(container_pos) {
            if breaker.yields {
                for stack in container.slots.into_iter().flatten() {
                    self.deliver_break_stack(breaker.collector, event.pos, stack, (sky, blk));
                }
            }
        }
        events.world.block_broken.push(BlockBrokenEvent {
            pos: event.pos,
            block: event.block,
            normal: breaker.normal,
            tint: broken_tint,
        });
        if let Some(stacks) = drops_override.filter(|_| breaker.yields) {
            for mut stack in stacks {
                if carries_to_item(event.block, stack.item)
                    && stack.variant == petramond_world::item::VariantId::NONE
                {
                    stack.variant = carry_variant;
                }
                self.deliver_break_stack(breaker.collector, event.pos, stack, (sky, blk));
            }
        } else if event.harvested {
            let stacks = match part_drops {
                Some(stacks) => stacks,
                None => self.roll_drops(event.block, carry_variant),
            };
            for stack in stacks {
                self.deliver_break_stack(breaker.collector, event.pos, stack, (sky, blk));
            }
        }
        self.mods.emit(PostEvent::BlockBroken {
            pos: event.pos,
            block: event.block,
            harvested: event.harvested,
            natural: false,
            player: breaker.actor.player(),
        });
        self.push_noise_from(breaker.actor, event.pos, crate::mob::NoiseKind::BlockBroken);
        true
    }

    fn deliver_break_stack(
        &mut self,
        collector: Option<u64>,
        pos: IVec3,
        stack: ItemStack,
        light: (u8, petramond_world::light::BlockLight6),
    ) {
        let mut rest = Some(stack);
        let mobs = self.world.mobs_mut();
        if let Some(container) = collector.and_then(|id| mobs.container_mut(id)) {
            let len = container.slots.len();
            petramond_world::container::route_into(
                &mut rest,
                &mut container.slots,
                &vec![petramond_world::container::SlotSpec::default(); len],
                None,
            );
        }
        if let Some(stack) = rest {
            self.spawn_item_stack(pos, stack, light);
        }
    }

    pub fn process_natural_breaks(&mut self, events: &mut TickEvents) {
        for (pos, block) in self.world.take_natural_breaks() {
            if block.has_tag(petramond_world::block::BlockTag::BED) {
                self.validate_bed_spawn();
            }
            events.world.block_broken.push(BlockBrokenEvent {
                pos,
                block,
                normal: None,
                tint: None,
            });
            let (sky, blk) = self
                .world
                .data()
                .dynamic_light_at_world(pos.x, pos.y, pos.z);
            let harvested = petramond_world::mining::harvests(block, None);
            if harvested {
                self.spawn_drops(
                    pos,
                    block,
                    (sky, blk),
                    petramond_world::item::VariantId::NONE,
                );
            }
            self.mods.emit(PostEvent::BlockBroken {
                pos,
                block,
                harvested,
                natural: true,
                player: None,
            });
        }
    }

    pub fn carry_variant_at(
        &self,
        pos: IVec3,
        block: Block,
        part: petramond_world::block::CellPart,
    ) -> petramond_world::item::VariantId {
        let carry = block.carry();
        if carry.is_empty() {
            return petramond_world::item::VariantId::NONE;
        }
        let mut map = petramond_world::item::variant::VariantMap::new();
        for &key in carry {
            let stored = petramond_world::block::part_kv_key(key, part);
            if let Some(v) = self.world.data().cell_kv_get(pos.x, pos.y, pos.z, &stored) {
                map.insert(key.to_owned(), v.to_vec());
            }
        }
        if map.is_empty() {
            return petramond_world::item::VariantId::NONE;
        }
        petramond_world::item::variant::intern(&map).unwrap_or_else(|e| {
            log::warn!("carry at {pos:?}: {e} — drop loses its data");
            petramond_world::item::VariantId::NONE
        })
    }

    pub fn part_drop_stacks(
        &self,
        pos: IVec3,
        parts: &[(petramond_world::block::CellPart, Block)],
    ) -> Vec<petramond_world::item::ItemStack> {
        let mut stacks: Vec<petramond_world::item::ItemStack> = Vec::new();
        for &(part, block) in parts {
            let item = petramond_world::item::ItemType::from_block(block);
            if item == petramond_world::item::ItemType::Air {
                continue;
            }
            let variant = self.carry_variant_at(pos, block, part);
            match stacks
                .iter_mut()
                .find(|s| s.item == item && s.variant == variant)
            {
                Some(s) => {
                    s.count = s.count.saturating_add(1).min(item.max_stack_size());
                }
                None => stacks.push(petramond_world::item::ItemStack::with_variant(
                    item, 1, variant,
                )),
            }
        }
        stacks
    }

    pub fn spawn_drops(
        &mut self,
        pos: IVec3,
        block: Block,
        light: (u8, petramond_world::light::BlockLight6),
        carry_variant: petramond_world::item::VariantId,
    ) {
        for stack in self.roll_drops(block, carry_variant) {
            self.spawn_item_stack(pos, stack, light);
        }
    }

    fn roll_drops(
        &mut self,
        block: Block,
        carry_variant: petramond_world::item::VariantId,
    ) -> Vec<ItemStack> {
        let mut stacks = Vec::new();
        for d in block.drop_spec().drops {
            let chance_seed = self.seeds.draw();
            if d.chance < 1.0 && crate::entity::hash01(chance_seed as u64) >= d.chance {
                continue;
            }
            let count_seed = self.seeds.draw();
            let count = if d.min >= d.max {
                d.min
            } else {
                let r = crate::entity::hash01(count_seed as u64);
                let span = (d.max - d.min + 1) as f32;
                (d.min + (r * span) as u8).min(d.max)
            };
            if count == 0 {
                continue;
            }
            let mut stack = ItemStack::new(d.item, count);
            if carries_to_item(block, d.item) {
                stack.variant = carry_variant;
            }
            stacks.push(stack);
        }
        stacks
    }

    pub(super) fn spawn_item_stack(
        &mut self,
        pos: IVec3,
        stack: ItemStack,
        (sky, blk): (u8, petramond_world::light::BlockLight6),
    ) {
        if stack.is_empty() {
            return;
        }
        let centre = petramond_math::world_pos::WorldPos::block_center(pos);
        let mut drop = DroppedItem::new(centre, stack, self.seeds.draw());
        drop.skylight = sky;
        drop.blocklight = blk;
        self.world.spawn_item(drop);
    }
}

fn carries_to_item(block: Block, item: petramond_world::item::ItemType) -> bool {
    item == petramond_world::item::ItemType::from_block(block)
        || (!block.carry().is_empty()
            && item
                .as_block()
                .is_some_and(|target| target.carry() == block.carry()))
}
