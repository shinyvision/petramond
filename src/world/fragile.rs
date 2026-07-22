//! The fragile-block break behaviour: plants and torches that cannot stand once the
//! support they rest on is gone.
//!
//! Lives here in `world` (not `block`) for the same reason water does — it drives the
//! world tick scheduler ([`World::schedule_block_tick`]) and the natural-break hand-off
//! ([`World::note_block_destroyed`]), world internals a `block`-side behaviour can't
//! reach — while still implementing the `block`-defined [`BlockBehavior`]. Carried by
//! every block tagged [`BlockTag::FRAGILE`](crate::block::BlockTag::FRAGILE) (see
//! `block::data`): the tag is the categorisation (the water sim reads it to know which
//! cells it may flow into), this behaviour is what such a block DOES when its support
//! changes.

use crate::block::{Block, BlockBehavior};
use crate::mathh::IVec3;

use super::store::World;

/// Ticks a now-unsupported fragile block waits before it breaks: the next tick. The
/// break resolves on the deterministic game tick *after* the change that undercut it,
/// never mid-frame — the same scheduled-tick model water uses, so a chain of supports
/// collapsing falls one layer per tick instead of all at once.
const FRAGILE_BREAK_DELAY: u64 = 1;

/// Break behaviour for fragile blocks (the cross-plants and the torch). A neighbour
/// change that takes away the block's support schedules its break for the next tick;
/// the scheduled tick re-checks (the support may have returned, or the cell may now hold
/// something else) and, only if the block is still fragile and still unsupported,
/// shatters it — dropping and bursting exactly as a player's hand-break would (see
/// [`World::note_block_destroyed`]).
pub struct Fragile;

impl BlockBehavior for Fragile {
    fn key(&self) -> &'static str {
        "fragile"
    }

    fn neighbor_update(&self, world: &mut World, pos: IVec3) {
        // Dispatch already read this cell as the fragile block; re-read to learn which
        // one (a torch derives its support sideways, a plant from the block below).
        let block = Block::from_id(world.chunk_block(pos.x, pos.y, pos.z));
        if !world.fragile_supported(pos, block) {
            world.schedule_block_tick(pos, FRAGILE_BREAK_DELAY);
        }
    }

    fn scheduled_tick(&self, world: &mut World, pos: IVec3) {
        let block = Block::from_id(world.chunk_block(pos.x, pos.y, pos.z));
        // The cell may have changed since the break was scheduled (mined, replaced, or
        // re-supported); only break a still-fragile, still-unsupported block.
        if !block.is_fragile() || world.fragile_supported(pos, block) {
            return;
        }
        // Shatter it as a natural break — drops + burst, exactly as a hand-break.
        world.break_block_naturally(pos);
    }
}

/// The fragile singleton a row points at (`behavior: &behavior::FRAGILE`).
pub static FRAGILE: Fragile = Fragile;

impl World {
    /// The cell that must stay solid to hold up non-torch fragile blocks.
    fn fragile_ground_cell(&self, pos: IVec3) -> IVec3 {
        pos - IVec3::new(0, 1, 0)
    }

    /// Whether the fragile block at `pos` still has something to stand on: plants use
    /// the old full-opaque ground rule, torches and ladders use the same mounted-face
    /// test as their placement, and lowered-cube covers (the snow layer and any pack
    /// block shaped like it) rest on ANY full collision cube — a dusting sits on
    /// leaves or glass just as well as on soil, while stairs, slabs, and model blocks
    /// still shed it (per Rachel: snow stays on any full block, and canopy snow must
    /// not shatter the tick after the weather mod lays it).
    fn fragile_supported(&self, pos: IVec3, block: Block) -> bool {
        if block.shape_family() == crate::block::ShapeFamily::Torch {
            return self.torch_supported_at(pos, self.torch_placement(pos));
        }
        if block.shape_family() == crate::block::ShapeFamily::Ladder {
            return self.ladder_supported_at(pos, block.panel_facing());
        }
        let s = self.fragile_ground_cell(pos);
        if block.shape_family() == crate::block::ShapeFamily::LoweredCube {
            return super::query::full_unit_cube(self.collision_boxes_at(s.x, s.y, s.z));
        }
        self.physics_block(s.x, s.y, s.z).is_opaque()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_state::{StairHalf, StairState};
    use crate::chunk::{Chunk, ChunkPos};
    use crate::crafting::Recipes;
    use crate::facing::Facing;
    use crate::torch::TorchPlacement;

    /// A world with one empty loaded chunk at the origin.
    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    fn run_ticks(w: &mut World, n: u32) {
        let r = Recipes::default();
        for _ in 0..n {
            w.game_tick(&r);
        }
    }

    fn block(w: &World, p: IVec3) -> Block {
        Block::from_id(w.chunk_block(p.x, p.y, p.z))
    }

    #[test]
    fn a_plant_breaks_the_tick_after_its_support_is_dug_away() {
        let mut w = world();
        let ground = IVec3::new(8, 64, 8);
        let plant = IVec3::new(8, 65, 8);
        w.set_block_world(ground.x, ground.y, ground.z, Block::Dirt);
        w.set_block_world(plant.x, plant.y, plant.z, Block::Poppy);
        run_ticks(&mut w, 2); // settle: supported, nothing happens
        assert_eq!(block(&w, plant), Block::Poppy);

        // Dig the support out: the flower is scheduled, then breaks on the next tick.
        w.set_block_world(ground.x, ground.y, ground.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            block(&w, plant),
            Block::Air,
            "unsupported flower must break"
        );
        // ...and it was handed to the presentation layer as a hand-style break.
        let breaks = w.take_natural_breaks();
        assert!(
            breaks.iter().any(|&(p, b)| p == plant && b == Block::Poppy),
            "the broken flower was recorded for its drop + particle burst",
        );
    }

    #[test]
    fn a_cactus_breaks_the_tick_after_the_sand_under_it_is_dug() {
        // The cactus is fragile just like the dead bush: undermine it and it shatters.
        let mut w = world();
        let sand = IVec3::new(8, 64, 8);
        let cactus = IVec3::new(8, 65, 8);
        w.set_block_world(sand.x, sand.y, sand.z, Block::Sand);
        w.set_block_world(cactus.x, cactus.y, cactus.z, Block::Cactus);
        run_ticks(&mut w, 2); // settle: the sand holds it up, nothing happens
        assert_eq!(block(&w, cactus), Block::Cactus);

        // Dig the sand out: the cactus is scheduled, then breaks on the next tick.
        w.set_block_world(sand.x, sand.y, sand.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            block(&w, cactus),
            Block::Air,
            "an undermined cactus must break"
        );
        let breaks = w.take_natural_breaks();
        assert!(
            breaks
                .iter()
                .any(|&(p, b)| p == cactus && b == Block::Cactus),
            "the broken cactus was recorded for its drop + particle burst",
        );
    }

    #[test]
    fn a_supported_plant_survives_a_change_beside_it() {
        let mut w = world();
        w.set_block_world(8, 64, 8, Block::Dirt);
        w.set_block_world(8, 65, 8, Block::Poppy);
        // A change next to the plant (its support untouched) must not break it.
        w.set_block_world(9, 65, 8, Block::Dirt);
        run_ticks(&mut w, 3);
        assert_eq!(block(&w, IVec3::new(8, 65, 8)), Block::Poppy);
        assert!(w.take_natural_breaks().is_empty());
    }

    #[test]
    fn a_wall_torch_breaks_when_the_wall_it_leans_on_is_removed() {
        let mut w = world();
        let torch = IVec3::new(8, 65, 8);
        // A West-leaning torch is mounted on the wall to its +X (see `TorchPlacement`):
        // its support is sideways, the one non-data-driven case.
        let wall = TorchPlacement::West.support_cell(torch);
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        w.set_block_world(torch.x, torch.y, torch.z, Block::Torch);
        w.insert_torch(torch, TorchPlacement::West);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, torch), Block::Torch, "held up by its wall");

        // Mine the wall: the torch loses its sideways support and breaks next tick.
        w.set_block_world(wall.x, wall.y, wall.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            block(&w, torch),
            Block::Air,
            "a wall torch falls with its wall"
        );
        let breaks = w.take_natural_breaks();
        assert!(breaks.iter().any(|&(p, b)| p == torch && b == Block::Torch));
    }

    #[test]
    fn a_ladder_breaks_the_tick_after_its_wall_is_mined() {
        use crate::facing::Facing;
        let mut w = world();
        let ladder = IVec3::new(8, 65, 8);
        // An east-facing ladder (its own block row) hangs on the wall to its west.
        let wall = crate::ladder::support_cell(ladder, Facing::East);
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        w.set_block_world(ladder.x, ladder.y, ladder.z, Block::LadderEast);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, ladder), Block::LadderEast, "held up by its wall");

        // Mine the wall: the ladder loses its support and breaks on the next tick
        // (the same announce → scheduled-break cadence as the wall torch above).
        w.set_block_world(wall.x, wall.y, wall.z, Block::Air);
        run_ticks(&mut w, 2);
        assert_eq!(
            block(&w, ladder),
            Block::Air,
            "a ladder falls with its wall on the following tick"
        );
        let breaks = w.take_natural_breaks();
        assert!(
            breaks
                .iter()
                .any(|&(p, b)| p == ladder && b == Block::LadderEast),
            "the broken ladder was recorded for its drop + particle burst",
        );
    }

    #[test]
    fn a_snow_layer_rests_on_any_full_cube_but_sheds_off_partial_shapes() {
        // Both sites stay >= SIM_READ_REACH cells from the lone chunk's
        // borders, or the streaming-finality guard drops the scheduled break.
        let mut w = world();
        // Leaves are a full collision cube without being opaque: canopy snow
        // must persist (the weather mod lays it there; it used to shatter on
        // the placement's own block update).
        w.set_block_world(7, 64, 8, Block::OakLeaves);
        w.set_block_world(7, 65, 8, Block::SnowLayer);
        run_ticks(&mut w, 3);
        assert_eq!(
            block(&w, IVec3::new(7, 65, 8)),
            Block::SnowLayer,
            "canopy snow must persist on leaves"
        );

        // A stair is not a full cube: the layer sheds on the next tick.
        let stair = IVec3::new(9, 64, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));
        w.set_block_world(9, 65, 8, Block::SnowLayer);
        run_ticks(&mut w, 3);
        assert_eq!(
            block(&w, IVec3::new(9, 65, 8)),
            Block::Air,
            "stair-top snow must shed"
        );
    }

    #[test]
    fn a_wall_torch_on_a_stair_flat_side_survives_support_rechecks() {
        let mut w = world();
        let stair = IVec3::new(8, 66, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));
        let torch = stair - IVec3::X;
        w.set_block_world(torch.x, torch.y, torch.z, Block::Torch);
        w.insert_torch(torch, TorchPlacement::West);
        run_ticks(&mut w, 2);
        assert_eq!(block(&w, torch), Block::Torch, "stair back holds torch");
    }
}
