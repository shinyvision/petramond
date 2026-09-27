//! Saplings. A cross-plant that roots in soil, shatters when its ground goes, and grows into its
//! tree over a few random ticks.
//!
//! Why it's in `world` not `block`: growth runs a worldgen `Feature` against the live world through
//! a validating overlay and commits across chunk borders. Block-side behaviors can't reach that
//! far. Still implements `BlockBehavior` and sits in the registry, so a data row just says
//! `behavior::SAPLING`.
//!
//! A block gets one behavior. `neighbor_update` just calls [`FRAGILE`] (breaks when dug out, like a
//! flower). `random_tick` does the growth.
//!
//! Each growth stage is its own block row, all look identical. Each random tick has a 50% chance
//! to advance to `next_stage`. On the final stage, a successful roll grows the tree from
//! `grows_into` instead, a weighted pick over `features.json` keys. Oak's 20% grand-oak chance is
//! just row data, validated in `block::load`.
//!
//! Tree needs anchored roots (`is_anchored` gate, no floating trees off cliffs). Every block it
//! wants to place has to be air, or a log/leaves/fragile plant it can pass through. Logs stay put,
//! leaves and plants get replaced, so grass tufts don't block root splay. Anything else in the
//! footprint and the sapling waits to try again later.

use crate::world::ServerWorld;
use crate::world::WorldData;
use std::collections::HashMap;

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::section::SectionSummary;
use petramond_worldgen::growth::{ConfiguredFeature, FeatureCtx, VoxelSink};
use petramond_worldgen::FeatureRng;

use super::fragile::FRAGILE;

const SAPLING_SALT: u64 = 0x0000_5A91_1A6E_0000;

const ADVANCE_CHANCE: f32 = 0.5;

pub struct Sapling;

impl crate::world::engine_behavior::EngineBlockBehavior for Sapling {
    fn random_tick(&self, world: &mut ServerWorld, pos: IVec3) {
        let salt = SAPLING_SALT ^ world.current_tick();
        let mut rng = FeatureRng::positional(world.data.seed, salt, pos.x, pos.y, pos.z);
        if !rng.chance(ADVANCE_CHANCE) {
            return;
        }
        let sapling = Block::from_id(world.data.chunk_block(pos.x, pos.y, pos.z));
        match sapling.next_stage() {
            Some(next) => {
                world.set_block_world(pos.x, pos.y, pos.z, next);
            }
            None => world.grow_sapling(pos, sapling, &mut rng),
        }
    }

    fn neighbor_update(&self, world: &mut ServerWorld, pos: IVec3) {
        FRAGILE.neighbor_update(world, pos);
    }
}

pub static SAPLING: Sapling = Sapling;

fn pick_growth(
    choices: &[(&'static str, f32)],
    rng: &mut FeatureRng,
) -> Option<&'static ConfiguredFeature> {
    let (last, head) = choices.split_last()?;
    let mut remaining: f32 = choices.iter().map(|c| c.1).sum();
    let mut picked = last.0;
    for &(key, weight) in head {
        if rng.chance(weight / remaining) {
            picked = key;
            break;
        }
        remaining -= weight;
    }
    petramond_worldgen::growth::feature_by_name(picked)
}

impl ServerWorld {
    /// Grows sapling at `pos` into tree from `grows_into`.
    ///
    /// Anchor check uses live world - oaks don't root over a drop, same as worldgen. Actual growth
    /// happens in an overlay, live world untouched.
    ///
    /// Commit needs every cell to currently be air, log, leaves or fragile plant (sapling's own
    /// cell excepted - trunk takes that) and chunk loaded. Known-empty-sky sections count as
    /// loaded.
    ///
    /// Trunk barges through logs and leaves. Roots plow through grass and flowers same as worldgen,
    /// so a tuft never blocks them. Anything else blocks the whole growth.
    ///
    /// Logs survive commit, leaves and plants don't. No fit means sapling just sits and tries again
    /// on a later tick.
    fn grow_sapling(&mut self, pos: IVec3, sapling: Block, rng: &mut FeatureRng) {
        let Some(cf) = pick_growth(sapling.grows_into(), rng) else {
            return;
        };

        let anchored = cf.feature.is_anchored(
            &mut |wx, wz| match self.data.block_if_loaded(wx, pos.y - 1, wz) {
                Some(b)
                    if b != Block::Air
                        && b != Block::Water
                        && !b.is_leaves()
                        && !b.is_fragile() =>
                {
                    pos.y
                }
                _ => i32::MIN,
            },
            pos,
            *rng,
        );
        if !anchored {
            return;
        }

        let writes = {
            let mut sink = GrowSink::new(&self.data);
            let mut ctx = FeatureCtx::new(&mut sink);
            cf.feature.generate(
                &mut ctx,
                &mut |p: IVec3| match self.data.block_if_loaded(p.x, p.y, p.z) {
                    None => true,
                    Some(b) => {
                        b == Block::Air
                            || b.is_log()
                            || b.is_leaves()
                            || b.is_fragile()
                            || b.is_snow_cover()
                    }
                },
                pos,
                rng,
            );
            sink.overlay
        };

        for &cell in writes.keys() {
            if cell == pos {
                continue;
            }
            match self.data.block_if_loaded(cell.x, cell.y, cell.z) {
                Some(b)
                    if b == Block::Air
                        || b.is_log()
                        || b.is_leaves()
                        || b.is_fragile()
                        || b.is_snow_cover() => {}
                Some(_) => return,
                None => match WorldData::split_world(cell.x, cell.y, cell.z) {
                    Some((sp, ..)) if self.data.section_summary(sp) == SectionSummary::Empty => {}
                    _ => return,
                },
            }
        }

        for (cell, block) in writes {
            if cell != pos
                && self
                    .data
                    .block_if_loaded(cell.x, cell.y, cell.z)
                    .is_some_and(Block::is_log)
            {
                continue;
            }
            self.set_block_world(cell.x, cell.y, cell.z, block);
        }
    }
}

struct GrowSink<'a> {
    world: &'a WorldData,
    overlay: HashMap<IVec3, Block>,
}

impl<'a> GrowSink<'a> {
    fn new(world: &'a WorldData) -> Self {
        Self {
            world,
            overlay: HashMap::new(),
        }
    }
}

impl VoxelSink for GrowSink<'_> {
    fn get(&self, p: IVec3) -> Block {
        if let Some(&b) = self.overlay.get(&p) {
            return b;
        }
        self.world
            .block_if_loaded(p.x, p.y, p.z)
            .unwrap_or(Block::Air)
    }
    fn set(&mut self, p: IVec3, b: Block) {
        self.overlay.insert(p, b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::chunk::{Chunk, ChunkPos};
    use petramond_world::crafting::Recipes;

    use super::super::store::LoadTarget;

    fn world_with_grove() -> ServerWorld {
        let mut w = ServerWorld::new(1, 4);
        for cz in -1..=1 {
            for cx in -1..=1 {
                w.insert_chunk_for_test(ChunkPos::new(cx, cz), Chunk::new(cx, cz));
            }
        }
        w.data.last_load_target = Some(LoadTarget::new(0, 4, 0, 4));
        w
    }

    fn block(w: &ServerWorld, x: i32, y: i32, z: i32) -> Block {
        Block::from_id(w.data.chunk_block(x, y, z))
    }

    fn final_stage(mut b: Block) -> Block {
        while let Some(next) = b.next_stage() {
            b = next;
        }
        b
    }

    fn plant_oak(w: &mut ServerWorld) -> IVec3 {
        let pos = IVec3::new(8, 64, 8);
        for z in -4..=20 {
            for x in -4..=20 {
                w.set_block_world(x, 63, z, Block::Dirt);
            }
        }
        w.set_block_world(pos.x, pos.y, pos.z, final_stage(Block::OakSapling));
        pos
    }

    fn has_canopy(w: &ServerWorld) -> bool {
        (64..=90).any(|y| (3..=13).any(|x| (3..=13).any(|z| block(w, x, y, z).is_leaves())))
    }

    #[test]
    fn a_sapling_grows_into_a_tree_on_clear_ground() {
        let mut w = world_with_grove();
        let pos = plant_oak(&mut w);
        let mut rng = FeatureRng::from_state(0x1234_5678);
        w.grow_sapling(pos, final_stage(Block::OakSapling), &mut rng);

        assert!(
            block(&w, 8, 64, 8).is_log(),
            "trunk roots where the sapling stood"
        );
        assert!(has_canopy(&w), "a grown tree has a leaf canopy");
    }

    #[test]
    fn a_sapling_will_not_grow_when_a_solid_block_blocks_the_trunk() {
        let mut w = world_with_grove();
        let pos = plant_oak(&mut w);
        w.set_block_world(8, 65, 8, Block::Stone);
        let mut rng = FeatureRng::from_state(0x1234_5678);
        w.grow_sapling(pos, final_stage(Block::OakSapling), &mut rng);

        assert_eq!(
            block(&w, 8, 64, 8),
            final_stage(Block::OakSapling),
            "the blocked sapling stays"
        );
        assert_eq!(
            block(&w, 8, 65, 8),
            Block::Stone,
            "the obstruction is untouched"
        );
        assert!(!has_canopy(&w), "nothing of the tree was placed");
    }

    #[test]
    fn a_tree_grows_through_logs_already_in_the_way() {
        let mut w = world_with_grove();
        let pos = plant_oak(&mut w);
        w.set_block_world(8, 65, 8, Block::OakLog);
        let mut rng = FeatureRng::from_state(0x1234_5678);
        w.grow_sapling(pos, final_stage(Block::OakSapling), &mut rng);

        assert!(
            block(&w, 8, 64, 8).is_log(),
            "the sapling grew past the log"
        );
        assert_eq!(
            block(&w, 8, 65, 8),
            Block::OakLog,
            "the pre-existing log remains"
        );
        assert!(has_canopy(&w), "the tree still grew its canopy");
    }

    #[test]
    fn a_tree_grows_through_leaves_already_in_the_way() {
        let mut w = world_with_grove();
        let pos = plant_oak(&mut w);
        w.set_block_world(8, 65, 8, Block::BirchLeaves);
        w.set_block_world(7, 67, 8, Block::SpruceLeaves);
        let mut rng = FeatureRng::from_state(0x1234_5678);
        w.grow_sapling(pos, final_stage(Block::OakSapling), &mut rng);

        assert!(
            block(&w, 8, 64, 8).is_log(),
            "the sapling grew past the leaves"
        );
        assert!(has_canopy(&w), "the tree grew its canopy");
    }

    #[test]
    fn a_tree_grows_through_ground_cover() {
        let mut w = world_with_grove();
        let pos = plant_oak(&mut w);
        for (x, z) in [(6, 8), (10, 9), (8, 11), (12, 8), (5, 5)] {
            w.set_block_world(x, 64, z, Block::ShortGrass);
        }
        w.set_block_world(9, 64, 6, Block::Poppy);
        let mut rng = FeatureRng::from_state(0x1234_5678);
        w.grow_sapling(pos, final_stage(Block::OakSapling), &mut rng);

        assert!(
            block(&w, 8, 64, 8).is_log(),
            "ground cover must not block growth"
        );
        assert!(has_canopy(&w), "the tree grew its canopy");
    }

    #[test]
    fn a_sapling_waits_when_roots_would_hang() {
        let mut w = world_with_grove();
        let pos = IVec3::new(8, 64, 8);
        w.set_block_world(8, 63, 8, Block::Dirt);
        w.set_block_world(pos.x, pos.y, pos.z, final_stage(Block::OakSapling));
        let mut rng = FeatureRng::from_state(0x1234_5678);
        w.grow_sapling(pos, final_stage(Block::OakSapling), &mut rng);

        assert_eq!(
            block(&w, 8, 64, 8),
            final_stage(Block::OakSapling),
            "the unanchorable sapling stays"
        );
        assert!(!has_canopy(&w), "nothing of the tree was placed");
    }

    #[test]
    fn random_ticks_walk_the_stage_rows_and_grow_a_planted_sapling() {
        let mut w = world_with_grove();
        let pos = plant_oak(&mut w);
        w.set_block_world(pos.x, pos.y, pos.z, Block::OakSapling);
        let recipes = Recipes::default();
        let mut grew = false;
        for _ in 0..1_000_000 {
            w.game_tick(&recipes);
            let b = block(&w, pos.x, pos.y, pos.z);
            if b.is_log() {
                grew = true;
                break;
            }
            assert!(
                b.has_tag(petramond_world::block::BlockTag::SAPLING),
                "every intermediate stage is a sapling row, got {b:?}"
            );
        }
        assert!(grew, "a planted sapling was never grown by random ticks");
    }
}
