use crate::block::{Block, RenderShape};
use crate::entity::DroppedItem;
use crate::events::{BlockBreakPre, Outcome, PostEvent};
use crate::item::ItemStack;
use crate::mathh::{IVec3, Vec3};
use crate::mining::BreakEvent;
use crate::world::World;

use super::game::ServerGame;
use crate::game::tick::{BlockBrokenEvent, TickEvents, TICK_DT};

impl ServerGame {
    /// Mining, on the tick: advance the break timer against the block under the crosshair
    /// (sampled per frame into `look`) by [`TICK_DT`], and when a block finishes breaking,
    /// clear it, scatter any block-entity contents + harvested drops, and spawn the break
    /// burst. Frame-rate independent. Gated off (progress reset) while a screen owns input
    /// (`intent_gameplay` false) — that's `inventory_open` to the mining controller.
    /// The mining-dust flecks are presentation, emitted CLIENT-side per frame from
    /// this session's live mining state (`Game::tick_mining_dust`).
    pub(crate) fn tick_mining(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        // The held tool (None = bare hand) gates mining speed + whether drops fall.
        let tool = sess
            .player
            .inventory
            .selected()
            .and_then(|st| st.item.tool());
        let look = sess.look;
        if let Some(event) = sess.mining.update(
            TICK_DT,
            look.map(|t| t.block),
            sess.intent_break_held,
            !sess.intent_gameplay,
            &self.world,
            tool,
        ) {
            self.finish_player_break(s, event, events);
        }
    }

    /// Apply a finished player break: announce `block_break_pre` (cancel =
    /// unbreakable — the block stays; the spent mining progress is the cost), then
    /// clear the block, scatter block-entity contents + harvested drops, spawn the
    /// burst, and queue `block_broken`.
    pub(crate) fn finish_player_break(
        &mut self,
        s: usize,
        event: BreakEvent,
        events: &mut TickEvents,
    ) {
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
                return;
            }
        }
        events.player(s).broke_block = Some(event.block);
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
            // Sim-destroyed blocks are not cancellable (no pre event) in Phase 1;
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
