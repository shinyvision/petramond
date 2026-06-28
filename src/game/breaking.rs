use crate::block::{Block, RenderShape};
use crate::entity::DroppedItem;
use crate::item::ItemStack;
use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::{
    tick::{TickEvents, TICK_DT},
    Game, MINING_DUST_INTERVAL,
};

impl Game {
    /// Mining, on the tick: advance the break timer against the block under the crosshair
    /// (sampled per frame into `look`) by [`TICK_DT`], and when a block finishes breaking,
    /// clear it, scatter any block-entity contents + harvested drops, and spawn the break
    /// burst. Frame-rate independent. Gated off (progress reset) while a screen owns input
    /// (`intent_gameplay` false) — that's `inventory_open` to the mining controller.
    pub(super) fn tick_mining(&mut self, events: &mut TickEvents) {
        // The held tool (None = bare hand) gates mining speed + whether drops fall.
        let tool = self.player.inventory.selected().and_then(|s| s.item.tool());
        if let Some(event) = self.mining.update(
            TICK_DT,
            self.look.as_ref(),
            self.intent_break_held,
            !self.intent_gameplay,
            &self.world,
            tool,
        ) {
            events.broke_block = Some(event.block);
            let hit_normal = self
                .look
                .filter(|h| h.block == event.pos && h.normal != IVec3::ZERO)
                .map(|h| h.normal);
            let (light, warm) = break_light(&self.world, event.pos, hit_normal);
            // A bbmodel block breaks as a whole: removing any cell clears every footprint
            // cell (the 2×2×1 workbench vanishes as one object, drops one item below).
            if matches!(event.block.render_shape(), RenderShape::Model(_)) {
                self.world.remove_model_block(event.pos);
            } else if event.block.render_shape() == RenderShape::Door {
                // A door breaks as a whole: removing either cell clears both halves and
                // drops one door item (the `spawn_drops` below). Forget any swing too.
                if let Some(lower) =
                    self.world
                        .door_lower_cell(event.pos.x, event.pos.y, event.pos.z)
                {
                    self.door_swings.remove(&lower);
                }
                self.world.remove_door(event.pos);
            } else {
                self.world
                    .set_block_world(event.pos.x, event.pos.y, event.pos.z, Block::Air);
            }
            // A broken furnace scatters whatever it held, regardless of tool (the
            // furnace ITEM still needs a pickaxe — handled by spawn_drops below).
            if event.block == Block::Furnace {
                if let Some(f) = self.world.take_furnace(event.pos) {
                    for stack in [f.input, f.fuel, f.output].into_iter().flatten() {
                        self.spawn_item_stack(event.pos, stack, light);
                    }
                }
            } else if event.block == Block::Chest {
                // A broken chest scatters its whole contents, regardless of tool.
                if let Some(chest) = self.world.take_chest(event.pos) {
                    for stack in chest.slots.into_iter().flatten() {
                        self.spawn_item_stack(event.pos, stack, light);
                    }
                }
            } else if event.block == Block::Torch {
                // A torch has no contents — just forget its recorded orientation so
                // the freed cell carries no stale block-entity state.
                self.world.take_torch(event.pos);
            }
            // A bbmodel block has no block-atlas tile, so its burst samples its own
            // texture (the model atlas); every other block uses its face tiles.
            match event.block.render_shape() {
                RenderShape::Model(kind) => self
                    .particles
                    .spawn_break_burst_model(event.pos, kind, light, warm),
                _ => self
                    .particles
                    .spawn_break_burst_lit(event.pos, event.block, light, warm),
            }
            if event.harvested {
                self.spawn_drops(event.pos, event.block, light);
            }
        }

        // A small dust fleck every MINING_DUST_INTERVAL while actively breaking.
        if self.mining.is_mining() {
            if let Some(h) = self.look {
                self.mining_dust_t += TICK_DT;
                if self.mining_dust_t >= MINING_DUST_INTERVAL {
                    self.mining_dust_t = 0.0;
                    let block =
                        Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
                    let cell = h.block + h.normal;
                    let (light, warm) = self.world.dynamic_light_at_world(cell.x, cell.y, cell.z);
                    match block.render_shape() {
                        RenderShape::Model(kind) => self
                            .particles
                            .spawn_mining_model(h.block, h.normal, kind, light, warm),
                        _ => self
                            .particles
                            .spawn_mining_lit(h.block, h.normal, block, light, warm),
                    }
                }
            }
        } else {
            self.mining_dust_t = 0.0;
        }
    }

    /// Drain the blocks the world simulation destroyed this tick (fragile blocks that
    /// lost support or were washed away by water) and give each the same break a player
    /// would — the break-particle burst plus its rolled item drops. Particles are purely
    /// visual (Game-owned), so they're spawned here rather than inside the world tick; the
    /// drops materialise on this tick like every other entity. The block is already gone
    /// (the world cleared the cell), so light is sampled from the now-empty cell — which is
    /// what the burst should glow with.
    pub(super) fn process_natural_breaks(&mut self) {
        for (pos, block) in self.world.take_natural_breaks() {
            let (light, warm) = self.world.dynamic_light_at_world(pos.x, pos.y, pos.z);
            match block.render_shape() {
                RenderShape::Model(kind) => self
                    .particles
                    .spawn_break_burst_model(pos, kind, light, warm),
                _ => self
                    .particles
                    .spawn_break_burst_lit(pos, block, light, warm),
            }
            // Fragile blocks are all tier-0 hand-harvestable, so they drop exactly as a
            // hand-break would (short grass yields nothing, a flower/torch yields itself).
            self.spawn_drops(pos, block, light);
        }
    }

    pub(super) fn spawn_drops(&mut self, pos: IVec3, block: Block, skylight: u8) {
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
            drop.skylight = skylight;
            self.world.spawn_item(drop);
        }
    }

    /// Spawn `stack` as a dropped item at the centre of block `pos` (e.g. a broken
    /// furnace scattering its contents). No-op for an empty stack.
    fn spawn_item_stack(&mut self, pos: IVec3, stack: ItemStack, skylight: u8) {
        if stack.is_empty() {
            return;
        }
        let centre = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32) + Vec3::splat(0.5);
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let mut drop = DroppedItem::new(centre, stack, self.spawn_counter);
        drop.skylight = skylight;
        self.world.spawn_item(drop);
    }
}

/// Combined light + warm at the lit face of a just-broken block, for its break
/// particles: the mined face's `(combined, warm)`, or the brightest neighbour when
/// the face is unknown.
fn break_light(world: &World, pos: IVec3, normal: Option<IVec3>) -> (u8, u8) {
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
    .max_by_key(|(combined, _)| *combined)
    .unwrap_or((63, 0))
}
