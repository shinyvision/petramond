//! Saplings: a cross-plant that roots in soil, shatters when undermined like any
//! fragile plant, AND grows into its species' tree over a few random ticks.
//!
//! Lives here in `world` (not `block`) for the same reason water and fragile do:
//! the growth step reaches into world internals — it runs a worldgen `Feature`
//! against the LIVE world through a validating overlay and commits across chunk
//! borders — which a `block`-side behaviour can't. It still implements the
//! `block`-defined [`BlockBehavior`] and is re-exported in the behaviour registry,
//! so a data row reads `behavior::SAPLING`.
//!
//! A sapling is BOTH fragile and a grower, but a block has ONE behaviour, so this
//! one composes: its support hooks ([`neighbor_update`](Sapling::neighbor_update) /
//! [`scheduled_tick`](Sapling::scheduled_tick)) delegate to [`FRAGILE`] (the sapling
//! breaks when its ground is dug, exactly like a flower), while
//! [`random_tick`](Sapling::random_tick) drives the growth.
//!
//! Growth stages are DISTINCT BLOCK ROWS (visually identical): on each random
//! tick a sapling has a 50% chance to advance, and advancing swaps the cell to
//! the row's `next_stage` block. A successful roll on a FINAL stage row (no
//! `next_stage`) instead grows the tree its row's `grows_into` field names — a
//! weighted draw over `features.json` keys, so the species→tree link and the
//! oak's 20% grand-oak split are row data, validated at load (see
//! `block::load`). A tree only grows if its roots are anchored (the feature's
//! `is_anchored` gate — no floating trees off cliff edges) and every block it
//! would place lands in air, or passes through a log, leaves, or a fragile
//! plant already in the way (existing logs are kept; leaves and plants yield,
//! so grass tufts on the forest floor never block the oak root splay) — any
//! other block in the footprint refuses growth, and the sapling waits and
//! tries again later.

use std::collections::HashMap;

use crate::block::{Block, BlockBehavior};
use crate::mathh::IVec3;
use crate::section::SectionSummary;
use crate::worldgen::feature::{ConfiguredFeature, FeatureCtx, VoxelSink};
use crate::worldgen::rng::FeatureRng;

use super::fragile::FRAGILE;
use super::store::World;

/// Salt for the sapling growth RNG stream — distinct from the worldgen feature salt
/// so a grown tree and a worldgen tree at the same spot don't share a stream.
const SAPLING_SALT: u64 = 0x0000_5A91_1A6E_0000;

/// Per-random-tick probability that a sapling advances a stage — or, at the final
/// stage, attempts to grow. (The task's 50%.)
const ADVANCE_CHANCE: f32 = 0.5;

/// A sapling. See the module docs: fragile, and grows on random ticks.
pub struct Sapling;

impl BlockBehavior for Sapling {
    fn key(&self) -> &'static str {
        "sapling"
    }

    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut World, pos: IVec3) {
        // A fresh deterministic stream per (sapling, tick): the tick number folds
        // into the salt so the same cell rolls differently every tick, while the
        // result stays a pure function of (seed, tick, pos) — reproducible for the
        // deterministic multiplayer simulation.
        let salt = SAPLING_SALT ^ world.current_tick();
        let mut rng = FeatureRng::positional(world.seed, salt, pos.x, pos.y, pos.z);
        if !rng.chance(ADVANCE_CHANCE) {
            return;
        }
        let sapling = Block::from_id(world.chunk_block(pos.x, pos.y, pos.z));
        match sapling.next_stage() {
            // A growing stage advances by becoming the next stage's block row
            // — stage is plain block identity, riding the ordinary edit path
            // (save, replication, and mod reads all see it for free).
            Some(next) => {
                world.set_block_world(pos.x, pos.y, pos.z, next);
            }
            None => world.grow_sapling(pos, sapling, &mut rng),
        }
    }

    // A sapling is fragile: it shatters the tick after the soil under it is dug away
    // (or water floods its cell), exactly like a flower. Delegate both support hooks
    // to the shared FRAGILE behaviour rather than duplicate its schedule-and-break.
    fn neighbor_update(&self, world: &mut World, pos: IVec3) {
        FRAGILE.neighbor_update(world, pos);
    }
    fn scheduled_tick(&self, world: &mut World, pos: IVec3) {
        FRAGILE.scheduled_tick(world, pos);
    }
}

/// The sapling singleton a row points at (`behavior: &behavior::SAPLING`).
pub static SAPLING: Sapling = Sapling;

/// Weighted draw over a final stage row's `grows_into` choices. The last entry
/// is the fallback and draws nothing, so a single-choice row consumes no RNG
/// and the oak's two-entry `[big w1, small w4]` list draws exactly one
/// `chance(0.2)` — the pre-row-data streams, preserved. Load validation
/// guarantees every key resolves; a row with no choices (unreachable through
/// the loader) grows nothing rather than defaulting to some species.
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
    crate::worldgen::data::features::by_name(picked)
}

impl World {
    /// Try to grow the final-stage sapling at `pos` (block `sapling`) into its tree.
    /// Picks the tree from the row's `grows_into` choices, checks the feature's
    /// ground-anchoring gate against the live
    /// world (the oaks refuse a site where a root would hang over a drop — the
    /// same no-floating-trees rule worldgen applies), generates the tree into
    /// an overlay over the LIVE world, and commits ONLY IF every cell the tree
    /// would change is currently air, a log, leaves, or a fragile plant — the
    /// sapling's own cell (which the trunk replaces) excepted — and lies in a
    /// loaded chunk (an absent section that is known empty sky counts as
    /// loaded air). So a tree grows up through any logs OR leaves already in
    /// its way, and its roots plow through grass tufts and flowers exactly as
    /// worldgen trees do (the huge oak root splay would otherwise be blocked
    /// by any tuft on a forest floor); only some other block (terrain, water,
    /// a built block) refuses the growth. On commit, pre-existing LOGS are
    /// kept (a tree never overwrites a log already there), while leaves and
    /// plants yield. If the tree doesn't fit, the sapling is left as-is to try
    /// again on a later tick.
    fn grow_sapling(&mut self, pos: IVec3, sapling: Block, rng: &mut FeatureRng) {
        let Some(cf) = pick_growth(sapling.grows_into(), rng) else {
            return;
        };

        // Ground-anchoring gate, mirroring worldgen's accepted-origin check:
        // a root ground cell is anchored when the block directly below it is
        // something solid to grip (not air/water, not leaves, not a fragile
        // plant that will shatter). The probe reports the surface as the
        // sapling's level when anchored, and as unreachable when not, which is
        // exactly the `surf ≥ origin.y - 1` contract `is_anchored` checks.
        // Runs on an rng COPY so `generate` still sees the post-pick stream.
        let anchored = cf.feature.is_anchored(
            &mut |wx, wz| match self.block_if_loaded(wx, pos.y - 1, wz) {
                Some(b)
                    if b != Block::Air && b != Block::Water && !b.is_leaves() && !b.is_fragile() =>
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

        // Generate into an overlay: reads fall through to the world (and the
        // feature's own earlier writes), every write lands in `overlay`, and the
        // world itself is untouched until we decide the tree fits. The sink borrows
        // `self` immutably only for this block; the owned overlay outlives it.
        let writes = {
            let mut sink = GrowSink::new(self);
            let mut ctx = FeatureCtx::new(&mut sink);
            cf.feature.generate(&mut ctx, pos, rng);
            sink.overlay
        };

        // Validate: every changed cell must be air, a log, leaves, or a
        // fragile plant — in a LOADED chunk. The origin holds the sapling
        // itself, which the trunk consumes — skip it. Only some OTHER block
        // (terrain, water, a built block) or an unloaded cell refuses growth.
        for &cell in writes.keys() {
            if cell == pos {
                continue;
            }
            match self.block_if_loaded(cell.x, cell.y, cell.z) {
                Some(b) if b == Block::Air || b.is_log() || b.is_leaves() || b.is_fragile() => {}
                Some(_) => return,
                // An absent SECTION whose generated summary is empty sky is
                // still growable air — the commit's `set_block_world`
                // materializes it on demand. Any other absent cell (unloaded
                // column, saved-but-unloaded, solid/water summary) refuses
                // growth as before.
                None => match Self::split_world(cell.x, cell.y, cell.z) {
                    Some((sp, ..)) if self.section_summary(sp) == SectionSummary::Empty => {}
                    _ => return,
                },
            }
        }

        // Commit: write each cell, but never replace a log that was already there
        // (the tree grows around it). The sapling's own cell is not a pre-existing
        // log, so the trunk base lands there and the sapling is consumed.
        for (cell, block) in writes {
            if cell != pos
                && self
                    .block_if_loaded(cell.x, cell.y, cell.z)
                    .is_some_and(Block::is_log)
            {
                continue;
            }
            self.set_block_world(cell.x, cell.y, cell.z, block);
        }
    }
}

/// A [`VoxelSink`] that runs a tree feature against the live world WITHOUT mutating
/// it: reads fall through to the world (or to the feature's own earlier writes), and
/// every write accumulates in `overlay` for the grower to validate and commit. So
/// the feature builds exactly as it would in worldgen, but "does it fit?" can inspect
/// the whole intended write set first.
struct GrowSink<'a> {
    world: &'a World,
    overlay: HashMap<IVec3, Block>,
}

impl<'a> GrowSink<'a> {
    fn new(world: &'a World) -> Self {
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
        // An unloaded / out-of-column cell reads as Air so the feature's air-only
        // predicates still write into it; validation separately aborts any growth
        // that reaches an unloaded chunk, so this never grows blindly past the edge.
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
    use crate::chunk::{Chunk, ChunkPos};
    use crate::crafting::Recipes;

    use super::super::store::LoadTarget;

    /// A world with a 3×3 of loaded chunks around the origin, the player centred on
    /// it (so its sections are eligible for random ticks), and a dirt floor under the
    /// centre — so a sapling at (8,64,8) is supported and any tree it grows stays in
    /// loaded chunks.
    fn world_with_grove() -> World {
        let mut w = World::new(1, 4);
        for cz in -1..=1 {
            for cx in -1..=1 {
                w.insert_chunk_for_test(ChunkPos::new(cx, cz), Chunk::new(cx, cz));
            }
        }
        w.last_load_target = Some(LoadTarget::new(0, 4, 0, 4));
        w
    }

    fn block(w: &World, x: i32, y: i32, z: i32) -> Block {
        Block::from_id(w.chunk_block(x, y, z))
    }

    /// The final growth-stage row of a sapling's chain — the row `grow_sapling`
    /// runs on (walked through the same `next_stage` links the behaviour uses).
    fn final_stage(mut b: Block) -> Block {
        while let Some(next) = b.next_stage() {
            b = next;
        }
        b
    }

    /// Plant an oak sapling at its FINAL growth stage at (8,64,8) on a dirt
    /// floor wide enough for the oak root splay's anchoring gate; return its
    /// position.
    fn plant_oak(w: &mut World) -> IVec3 {
        let pos = IVec3::new(8, 64, 8);
        for z in -4..=20 {
            for x in -4..=20 {
                w.set_block_world(x, 63, z, Block::Dirt);
            }
        }
        w.set_block_world(pos.x, pos.y, pos.z, final_stage(Block::OakSapling));
        pos
    }

    /// Is there any leaf in a generous box around the trunk (covers a small OR a
    /// giant oak's canopy, so the test doesn't care which the RNG picked).
    fn has_canopy(w: &World) -> bool {
        (64..=90).any(|y| (3..=13).any(|x| (3..=13).any(|z| block(w, x, y, z).is_leaves())))
    }

    #[test]
    fn a_sapling_grows_into_a_tree_on_clear_ground() {
        let mut w = world_with_grove();
        let pos = plant_oak(&mut w);
        let mut rng = FeatureRng::from_state(0x1234_5678);
        w.grow_sapling(pos, final_stage(Block::OakSapling), &mut rng);

        // The trunk roots where the sapling stood, with a canopy above — the sapling
        // is consumed and its (now meaningless) growth stage cleared.
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
        // Stone directly above the sapling sits in the trunk's path — a non-air,
        // non-log block the tree may not replace, so the growth is refused entirely.
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
        // A log where the trunk wants to go does NOT block growth (unlike the stone
        // above) and is itself kept — the tree grows up around it.
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
        // Leaves of ANY kind in the footprint are ignored by the growth check, just
        // like logs — a sapling under an existing canopy still matures. (A different
        // species' leaves, to prove "of any kind".)
        w.set_block_world(8, 65, 8, Block::BirchLeaves); // in the trunk's path
        w.set_block_world(7, 67, 8, Block::SpruceLeaves); // in the canopy's reach
        let mut rng = FeatureRng::from_state(0x1234_5678);
        w.grow_sapling(pos, final_stage(Block::OakSapling), &mut rng);

        assert!(
            block(&w, 8, 64, 8).is_log(),
            "the sapling grew past the leaves"
        );
        assert!(has_canopy(&w), "the tree grew its canopy");
    }

    /// Ground cover must never block growth: the oak root splay covers a wide
    /// disc of forest floor, and forest floors are littered with tufts and
    /// flowers — they yield to the tree exactly as they do in worldgen.
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

    /// The anchoring gate holds for player-grown trees too: on a lone dirt
    /// pillar the root splay hangs over air on every side, so the sapling
    /// waits instead of growing a floating tree.
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
        // End-to-end through the real tick path, from the BASE stage row: the
        // random-tick loop selects the sapling, walks it down its `next_stage`
        // chain (every intermediate block stays a sapling row), and finally
        // grows the tree. Deterministic for the fixed seed; the cap sits far
        // above the expected ticks.
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
                b.has_tag(crate::block::BlockTag::SAPLING),
                "every intermediate stage is a sapling row, got {b:?}"
            );
        }
        assert!(grew, "a planted sapling was never grown by random ticks");
    }
}
