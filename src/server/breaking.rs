use crate::block::{Block, RenderShape};
use crate::entity::DroppedItem;
use crate::events::{BlockBreakPre, Outcome, PostEvent};
use crate::item::ItemStack;
use crate::mathh::{IVec3, Vec3};
use crate::mining::{BreakEvent, MiningState};
use crate::world::World;

use super::game::ServerGame;
use crate::game::tick::{BlockBrokenEvent, TickEvents, TICK_DT};

/// How long a broken cell stays in `pending_break_ack` waiting for its lagged
/// `BreakFinished` (10 s — far beyond any real finish RTT). An expired entry
/// only means such a finish is denied with corrective cells, which reconciles
/// the client anyway; without the TTL an orphaned hold-path break (the client
/// released on the exact tick the server's timer crossed) grows the set for
/// the session's lifetime.
const BREAK_ACK_TTL_TICKS: u64 = 200;

impl ServerGame {
    /// Mining, on the tick: advance the break timer against the block under the
    /// crosshair while held, AND resolve client `BreakFinished` requests
    /// (duration/tool/reach validated).
    pub(crate) fn tick_mining(&mut self, s: usize, events: &mut TickEvents) {
        let now = self.world.current_tick();
        self.sessions[s]
            .pending_break_ack
            .retain(|_, broke_at| now.saturating_sub(*broke_at) <= BREAK_ACK_TTL_TICKS);
        if let Some(req) = self.sessions[s].pending_break_finished.take() {
            self.resolve_break_finished(s, req, events);
        }

        let tool = self.sessions[s]
            .player
            .inventory
            .selected()
            .and_then(|st| st.item.tool());
        let look = self.sessions[s].look;
        let break_held = self.sessions[s].intent_break_held;
        let inventory_open = !self.sessions[s].intent_gameplay;
        if let Some(event) = self.sessions[s].mining.update(
            TICK_DT,
            look.map(|t| t.block),
            break_held,
            inventory_open,
            &self.world,
            tool,
        ) {
            // Hold-path finish: if a TooFast BreakFinished was deferred for
            // this cell, accept it here. The initiator's own BlockBroken is
            // stripped only on EVIDENCE they presented locally — a deferred
            // finish flagged `predicted`. With no request latched the client
            // may never present at all: its per-frame timer can sit behind
            // this one (a sub-tick crosshair flicker resets it invisibly to
            // the tick-sampled look), and the break delta then cancels its
            // mining before it finishes — stripping on the old "finish must
            // be in flight" assumption made those breaks SILENT. A finish
            // that is merely in flight still presents exactly once: the
            // client's own presented cell suppresses the wire event until
            // the request resolves (the `predicted_presentation_cells`
            // belt in `game/replicated.rs`).
            let broken_pos = event.pos;
            let deferred = self.sessions[s]
                .deferred_break_finished
                .take_if(|d| d.pos == broken_pos);
            let presented = deferred.is_some_and(|d| d.predicted);
            if self.finish_player_break(s, event, events, presented) {
                if let Some(req) = deferred {
                    self.sessions[s].pending_break_ack.remove(&broken_pos);
                    self.push_action_outcome(s, req.request_id, true, None);
                }
            } else if let Some(req) = deferred {
                // `block_break_pre` cancelled after a deferred wait.
                self.deny_break_finished(
                    s,
                    req.request_id,
                    req.pos,
                    crate::net::protocol::ActionDenyReason::Denied,
                );
            }
        } else {
            // Mining abandoned / retargeted: any deferred finish for a cell
            // that is no longer the active target must deny + correct.
            self.abandon_deferred_break_if_stale(s);
        }
    }

    /// Drop a deferred TooFast finish whose cell is no longer being mined.
    fn abandon_deferred_break_if_stale(&mut self, s: usize) {
        let Some(req) = self.sessions[s].deferred_break_finished else {
            return;
        };
        let still_mining = self.sessions[s]
            .mining
            .progress()
            .is_some_and(|(target, _)| target == req.pos);
        if still_mining {
            return;
        }
        let req = self.sessions[s].deferred_break_finished.take().unwrap();
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

        // Reach from the claimed eye, BOUNDED by the F1 drift ring — the same
        // reference the look latch was validated against, so an accepted
        // mining target can't be denied at the finish by integration drift.
        let eye = crate::server::movement::reach_eye(&self.sessions[s]);
        if !crate::player::block_within_reach(eye, pos) {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::OutOfReach);
            return;
        }

        let block = Block::from_id(self.world.chunk_block(pos.x, pos.y, pos.z));
        // Hold-path may have already cleared the cell before a lagged
        // BreakFinished arrives. If THIS session broke it, accept — never
        // deny/restore (that re-spawns the block and invites a second break).
        if block == Block::Air {
            if self.sessions[s].pending_break_ack.remove(&pos).is_some() {
                if let Some(old) = self.sessions[s].deferred_break_finished.take() {
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
        if block.hardness() < 0.0 {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::Denied);
            return;
        }

        let auth_tool = self.sessions[s]
            .player
            .inventory
            .selected()
            .and_then(|st| st.item.tool());
        let claimed_tool = tool_item_id
            .map(crate::item::ItemType)
            .and_then(|it| it.tool());
        if claimed_tool != auth_tool {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::BadTool);
            return;
        }

        // Duration is validated against the SERVER'S OWN mining timer — the
        // hold-path `sess.mining` that accrues from the latched look +
        // break_held every tick. Client-reported time is never trusted.
        // Instant blocks (expected 0) need no observed window.
        //
        // TooFast is DEFERRED, not denied: the client's optimistic clear
        // stays, and when the hold-path timer finishes the same cell we
        // accept + strip presentation. Immediate deny would restore the
        // block then re-break it (double sound/burst on slow links).
        let expected = crate::mining::break_time(block, auth_tool);
        if expected > 0.0 {
            let observed = self.sessions[s]
                .mining
                .progress()
                .and_then(|(target, elapsed)| (target == pos).then_some(elapsed));
            if observed.is_none_or(|elapsed| elapsed + 3.0 * TICK_DT < expected) {
                // Supersede any prior deferred wait for another cell.
                if let Some(old) = self.sessions[s].deferred_break_finished.take() {
                    self.deny_break_finished(s, old.request_id, old.pos, ActionDenyReason::TooFast);
                }
                self.sessions[s].deferred_break_finished =
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
            harvested: crate::mining::harvests(block, auth_tool),
        };
        self.sessions[s].mining = MiningState::new();
        // A successful BreakFinished clears any deferred wait for this cell
        // (should be empty — we only defer when the window is short).
        if let Some(old) = self.sessions[s].deferred_break_finished.take() {
            if old.pos != pos {
                self.deny_break_finished(s, old.request_id, old.pos, ActionDenyReason::TooFast);
            } else {
                // Same cell already deferred — answer the older id as denied
                // (superseded by this accept path).
                self.push_action_outcome(s, old.request_id, false, Some(ActionDenyReason::TooFast));
            }
        }
        if self.finish_player_break(s, event, events, predicted) {
            self.sessions[s].pending_break_ack.remove(&pos);
            self.push_action_outcome(s, request_id, true, None);
        } else {
            self.deny_break_finished(s, request_id, pos, ActionDenyReason::Denied);
        }
    }

    /// Deny a `BreakFinished` and queue corrective cells for the claimed
    /// footprint so an optimistic clear cannot linger as phantom air.
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

    /// Authoritative footprint of `pos` into the session's corrective sync.
    fn queue_break_corrective_cells(&mut self, s: usize, pos: IVec3) {
        let cells = self.world.break_footprint_cells(pos);
        self.sessions[s].pending_corrective_cells.extend(cells);
    }

    /// Apply a finished player break: announce `block_break_pre` (cancel =
    /// unbreakable — the block stays; the spent mining progress is the cost), then
    /// clear the block, scatter block-entity contents + harvested drops, spawn the
    /// burst, and queue `block_broken`. Returns whether the block actually broke.
    ///
    /// `initiator_presented`: whether the breaking client is KNOWN to have
    /// played the break presentation locally (a finish request flagged
    /// `predicted`) — gates the echo strip. A client that never presented
    /// (frozen ledger, replica disagreement, or a hold-path finish that
    /// outpaced its timer) must still receive its `BlockBroken`.
    pub(crate) fn finish_player_break(
        &mut self,
        s: usize,
        event: BreakEvent,
        events: &mut TickEvents,
        initiator_presented: bool,
    ) -> bool {
        {
            let mut pre = BlockBreakPre {
                pos: event.pos,
                block: event.block,
                harvested: event.harvested,
            };
            let Self {
                world,
                sessions,
                bus,
                ..
            } = self;
            let sess = &mut sessions[s];
            if bus.block_break_pre(
                world,
                &mut sess.player,
                &mut sess.gui_state,
                events,
                &mut pre,
            ) == Outcome::Cancel
            {
                return false;
            }
        }
        events.player(s).broke_block = Some(event.block);
        // Echo rule: strip the initiator's BlockBroken only on evidence they
        // already presented it locally; a client that never presented still
        // needs the event, and one whose finish is in flight suppresses the
        // wire copy itself. Observers get the shared event either way.
        if initiator_presented {
            self.sessions[s].presented_breaks.push(event.pos);
        }
        // A lagged BreakFinished for this already-cleared cell must accept,
        // not deny/restore. Tick-stamped for the ack TTL.
        let now = self.world.current_tick();
        self.sessions[s].pending_break_ack.insert(event.pos, now);
        // Breaking a bed takes its spawn point with it — resolved BEFORE the
        // removal below clears the footprint metadata the group lookup needs.
        // Checked for EVERY session: any player can break another's spawn bed.
        if event.block.interaction() == crate::block::BlockInteraction::Sleep {
            self.clear_bed_spawn_at(event.pos);
        }
        let hit_normal = self.sessions[s]
            .look
            .filter(|h| h.block == event.pos && h.normal != IVec3::ZERO)
            .map(|h| h.normal);
        let (sky, blk, _warm) = break_light(&self.world, event.pos, hit_normal);
        let slab_drops = (event.block.render_shape() == RenderShape::Slab)
            .then(|| self.world.slab_drop_stacks_at(event.pos));
        // A mod container is keyed at the block's container anchor — resolved
        // BEFORE the removal below clears the model-group metadata the anchor
        // lookup needs (same ordering constraint as the bed spawn point).
        let container_pos = self.world.container_anchor(event.pos);
        // A bbmodel block breaks as a whole: removing any cell clears every footprint
        // cell (the 2×2×1 workbench vanishes as one object, drops one item below).
        if matches!(event.block.render_shape(), RenderShape::Model(_)) {
            self.world.remove_model_block(event.pos);
        } else if event.block.render_shape() == RenderShape::Door {
            // A door breaks as a whole: removing either cell clears both halves and
            // drops one door item (the `spawn_drops` below). The client-side swing
            // animation entry dies with it, dropped from the `block_broken` event
            // in `Game::apply_world_effects` (client-owned state).
            self.world.remove_door(event.pos);
        } else {
            self.world
                .set_block_world(event.pos.x, event.pos.y, event.pos.z, Block::Air);
        }
        // Forget the broken block's other entity records (machine state,
        // facing, torch orientation) in one generic sweep — no per-block
        // ladder to extend for the next facing-bearing block.
        self.world.forget_block_entity_records(event.pos);
        // ANY broken container block — chest, furnace, or a mod's — scatters
        // its whole contents, regardless of tool (the block ITEM's own drop
        // still gates on harvest via spawn_drops below).
        if let Some(container) = self.world.take_container(container_pos) {
            for stack in container.slots.into_iter().flatten() {
                self.spawn_item_stack(event.pos, stack, (sky, blk));
            }
        }
        // The break burst is presentation: queued as a world event and spawned
        // client-side after the tick (any observing client can do the same).
        events.world.block_broken.push(BlockBrokenEvent {
            pos: event.pos,
            block: event.block,
            normal: hit_normal,
        });
        if event.harvested {
            if let Some(stacks) = slab_drops {
                for stack in stacks {
                    self.spawn_item_stack(event.pos, stack, (sky, blk));
                }
            } else {
                self.spawn_drops(event.pos, event.block, (sky, blk));
            }
        }
        self.bus.emit(PostEvent::BlockBroken {
            pos: event.pos,
            block: event.block,
            harvested: event.harvested,
            natural: false,
        });
        self.push_block_noise(s, event.pos, crate::mob::NoiseKind::BlockBroken);
        true
    }

    /// Drain the blocks the world simulation destroyed this tick (fragile blocks that
    /// lost support or were washed away by water) and give each the same break a player
    /// would — the break-particle burst plus its rolled item drops. Particles are purely
    /// visual (Game-owned), so they're spawned here rather than inside the world tick; the
    /// drops materialise on this tick like every other entity. The block is already gone
    /// (the world cleared the cell), so light is sampled from the now-empty cell — which is
    /// what the burst should glow with.
    pub(crate) fn process_natural_breaks(&mut self, events: &mut TickEvents) {
        for (pos, block) in self.world.take_natural_breaks() {
            // The cell is already cleared, so the group base can't be derived;
            // re-checking the stored spawn bed still exists covers it.
            if block.interaction() == crate::block::BlockInteraction::Sleep {
                self.validate_bed_spawn();
            }
            events.world.block_broken.push(BlockBrokenEvent {
                pos,
                block,
                normal: None,
            });
            let (sky, blk, _warm) = self.world.dynamic_light_at_world(pos.x, pos.y, pos.z);
            // Fragile blocks are all tier-0 hand-harvestable, so they drop exactly as a
            // hand-break would (short grass yields nothing, a flower/torch yields itself).
            self.spawn_drops(pos, block, (sky, blk));
            // Sim-destroyed blocks are not cancellable (no pre event);
            // observers still hear about them.
            self.bus.emit(PostEvent::BlockBroken {
                pos,
                block,
                harvested: true,
                natural: true,
            });
        }
    }

    pub(crate) fn spawn_drops(&mut self, pos: IVec3, block: Block, (sky, blk): (u8, u8)) {
        let centre = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32) + Vec3::splat(0.5);
        for d in block.drop_spec().drops {
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            // Probabilistic drops (chance < 1, e.g. a leaf's 10% sapling) roll first;
            // a guaranteed drop (chance 1.0) always passes. Reuses the same seeded
            // hash the count roll uses, so the roll stays deterministic on the tick.
            if d.chance < 1.0 && crate::entity::hash01(self.spawn_counter as u64) >= d.chance {
                continue;
            }
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            // Roll a count in [min, max] (a fixed amount when min == max, e.g. the
            // 2–4 raw copper from copper ore).
            let count = if d.min >= d.max {
                d.min
            } else {
                let r = crate::entity::hash01(self.spawn_counter as u64);
                let span = (d.max - d.min + 1) as f32;
                (d.min + (r * span) as u8).min(d.max)
            };
            if count == 0 {
                continue;
            }
            let stack = ItemStack::new(d.item, count);
            let mut drop = DroppedItem::new(centre, stack, self.spawn_counter);
            drop.skylight = sky;
            drop.blocklight = blk;
            self.world.spawn_item(drop);
        }
    }

    /// Spawn `stack` as a dropped item at the centre of block `pos` (e.g. a broken
    /// furnace scattering its contents). No-op for an empty stack.
    fn spawn_item_stack(&mut self, pos: IVec3, stack: ItemStack, (sky, blk): (u8, u8)) {
        if stack.is_empty() {
            return;
        }
        let centre = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32) + Vec3::splat(0.5);
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let mut drop = DroppedItem::new(centre, stack, self.spawn_counter);
        drop.skylight = sky;
        drop.blocklight = blk;
        self.world.spawn_item(drop);
    }
}

/// Two-channel light + warm at the lit face of a just-broken block, for its break
/// particles: the mined face's `(sky6, block6, warm)`, or the brightest neighbour
/// (by combined `max(sky, block)`, matching the old single-channel pick) when the
/// face is unknown.
pub(crate) fn break_light(world: &World, pos: IVec3, normal: Option<IVec3>) -> (u8, u8, u8) {
    let at = |c: IVec3| world.dynamic_light_at_world(c.x, c.y, c.z);
    if let Some(n) = normal {
        return at(pos + n);
    }

    [
        IVec3::X,
        -IVec3::X,
        IVec3::Y,
        -IVec3::Y,
        IVec3::Z,
        -IVec3::Z,
    ]
    .into_iter()
    .map(|n| at(pos + n))
    .max_by_key(|&(sky, block, _)| sky.max(block))
    .unwrap_or((63, 0, 0))
}
