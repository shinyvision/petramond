//! Confinement detection: decide whether a mob has so little reachable space
//! that it should be treated as captive (e.g., penned). The probe is a bounded
//! flood-fill over navigation footholds, run infrequently per mob.

use rustc_hash::FxHashSet;

use crate::mathh::IVec3;
use crate::mob::path::{body_clear, body_layer_clear, is_navigation_foothold_with, PathParams};

/// Game ticks between confined-state re-evaluations for one mob.
pub const CHECK_INTERVAL: u8 = 60;

/// Horizontal search radius for the reachable-area probe, in cells.
pub const SEARCH_RADIUS: i32 = 8;

/// Minimum reachable cells before a mob is considered NOT confined.
/// A 5×5 pen has ~25 cells; a 6×6 pen has ~36. 33 catches common small pens
/// without flagging larger pastures.
pub const MIN_REACHABLE_CELLS: usize = 33;

/// Cardinal directions only; diagonals don't open new escape routes for a
/// confinement test and omitting them halves the probe work.
const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Returns `true` if the mob at `start` cannot reach at least
/// [`MIN_REACHABLE_CELLS`] navigable cells within [`SEARCH_RADIUS`] cells.
///
/// `solid`, `support`, and `water` match the pathfinder's semantics: `solid`
/// marks fully-blocked cells, `support` marks anything that can bear feet
/// (partial shapes included), water is a valid support surface but not
/// passable underwater. `step_allowed` is the navigator's per-edge sweep
/// (`nav::partial_step_gate`): the fill must agree with real routes — a lone
/// fence refuses the jump from below, while a step beside it opens the way
/// over (a pen with a step inside is genuinely escapable). Mobs that are not
/// on a foothold (swimming, mid-air, unsupported) are never reported as
/// confined.
pub fn is_confined(
    start: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    water: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
) -> bool {
    let foothold = |c: IVec3| is_navigation_foothold_with(c, params, solid, support, water);
    if !foothold(start) {
        return false;
    }

    let mut visited = FxHashSet::default();
    let mut queue = Vec::new();
    visited.insert(start);
    queue.push(start);

    while let Some(c) = queue.pop() {
        if visited.len() >= MIN_REACHABLE_CELLS {
            return false;
        }

        for (dx, dz) in DIRS {
            let side = c + IVec3::new(dx, 0, dz);
            if !in_search_cube(side, start) {
                continue;
            }

            // Jump up one block.
            let up = side + IVec3::Y;
            if !visited.contains(&up)
                && foothold(up)
                && step_allowed(c, up)
                && body_layer_clear(c + IVec3::Y * params.head_cells(), params, solid)
            {
                visited.insert(up);
                queue.push(up);
                continue;
            }

            // Flat step.
            if !visited.contains(&side) && foothold(side) && step_allowed(c, side) {
                visited.insert(side);
                queue.push(side);
                continue;
            }

            // Descend to the first foothold within max_drop.
            if body_clear(side, params, solid) {
                for dy in 1..=params.max_drop {
                    let down = side - IVec3::Y * dy;
                    if !in_search_cube(down, start) {
                        break;
                    }
                    if solid(down) {
                        break;
                    }
                    if !visited.contains(&down) && foothold(down) && step_allowed(c, down) {
                        visited.insert(down);
                        queue.push(down);
                        break;
                    }
                }
            }
        }
    }

    true
}

#[inline]
fn in_search_cube(c: IVec3, start: IVec3) -> bool {
    (c.x - start.x).abs() <= SEARCH_RADIUS
        && (c.y - start.y).abs() <= SEARCH_RADIUS
        && (c.z - start.z).abs() <= SEARCH_RADIUS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::mob::path::PathParams;
    use crate::world::World;

    fn flat_world(extra: impl FnOnce(&mut Chunk)) -> World {
        let mut world = World::new(0, 1);
        let mut chunk = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 63, z, Block::Grass);
                chunk.set_biome(x, z, crate::biome::Biome::Plains.id());
            }
        }
        extra(&mut chunk);
        world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
        world
    }

    /// Build a column of stone at chunk-local `(x,z)` from `y0` inclusive to `y1` exclusive.
    fn build_wall(chunk: &mut Chunk, x: i32, z: i32, y0: i32, y1: i32) {
        for y in y0..y1 {
            chunk.set_block(x as usize, y as usize, z as usize, Block::Stone);
        }
    }

    fn pen(extra: impl FnOnce(&mut Chunk)) -> (World, IVec3) {
        let world = flat_world(|chunk| {
            // 5×5 pen centred at chunk-local (8,64,8): walls at 5 and 11.
            for y in 64..68 {
                for i in 5..=11 {
                    build_wall(chunk, 5, i, y, y + 1);
                    build_wall(chunk, 11, i, y, y + 1);
                    build_wall(chunk, i, 5, y, y + 1);
                    build_wall(chunk, i, 11, y, y + 1);
                }
            }
            extra(chunk);
        });
        // Sheep stands in the centre of the pen.
        (world, IVec3::new(8, 64, 8))
    }

    fn params() -> PathParams {
        PathParams::for_body(2, 0.45)
    }

    fn check(world: &World, start: IVec3) -> bool {
        let solid = crate::mob::nav::nav_solid_fn(world);
        let support = crate::mob::nav::nav_support_fn(world, params().half_width);
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        let step = crate::mob::nav::partial_step_gate(world, params(), 1.4);
        is_confined(start, params(), &solid, &support, &water, &step)
    }

    #[test]
    fn open_field_is_not_confined() {
        let world = flat_world(|_| {});
        assert!(!check(&world, IVec3::new(8, 64, 8)));
    }

    #[test]
    fn small_pen_is_confined() {
        let (world, start) = pen(|_| {});
        assert!(check(&world, start), "5×5 pen should read as confined");
    }

    #[test]
    fn larger_pen_is_not_confined() {
        // 7×7 pen: plenty of cells above the threshold.
        let world = flat_world(|chunk| {
            for y in 64..68 {
                for i in 4..=12 {
                    build_wall(chunk, 4, i, y, y + 1);
                    build_wall(chunk, 12, i, y, y + 1);
                    build_wall(chunk, i, 4, y, y + 1);
                    build_wall(chunk, i, 12, y, y + 1);
                }
            }
        });
        assert!(!check(&world, IVec3::new(8, 64, 8)));
    }

    #[test]
    fn corridor_is_not_confined() {
        // Narrow but long corridor: the mob can travel far enough.
        let world = flat_world(|chunk| {
            for z in 0..CHUNK_SZ {
                for y in 64..68 {
                    build_wall(chunk, 6, z as i32, y, y + 1);
                    build_wall(chunk, 10, z as i32, y, y + 1);
                }
            }
        });
        assert!(!check(&world, IVec3::new(8, 64, 8)));
    }

    #[test]
    fn doorway_makes_pen_non_confined() {
        let (world, start) = pen(|chunk| {
            // Knock a one-block hole in the east wall.
            chunk.set_block(11, 64, 8, Block::Air);
            chunk.set_block(11, 65, 8, Block::Air);
        });
        assert!(!check(&world, start), "a door should break confinement");
    }

    /// A 5×5 pen of one-high fences centred at chunk-local (8,64,8): walls at
    /// 5 and 11. The flood fill must treat the fence as a wall even though a
    /// mob's physical jump could clear it.
    fn fence_pen(extra: impl FnOnce(&mut Chunk)) -> (World, IVec3) {
        let world = flat_world(|chunk| {
            for i in 5..=11 {
                for (x, z) in [(5, i), (11, i), (i, 5), (i, 11)] {
                    chunk.set_block(x, 64, z, Block::OakFence);
                }
            }
            extra(chunk);
        });
        (world, IVec3::new(8, 64, 8))
    }

    #[test]
    fn small_fence_pen_is_confined() {
        let (world, start) = fence_pen(|_| {});
        assert!(
            check(&world, start),
            "a one-high fence pen holds: the fill must not hop the fence"
        );
    }

    #[test]
    fn fence_pen_with_a_gap_is_not_confined() {
        let (world, start) = fence_pen(|chunk| {
            chunk.set_block(11, 64, 8, Block::Air);
        });
        assert!(!check(&world, start), "a gap in the fence breaks the pen");
    }

    #[test]
    fn fence_pen_with_a_step_up_inside_is_not_confined() {
        // A block inside the pen beside the fence is an honest escape route:
        // jump onto the block, walk over the fence top, drop outside.
        let (world, start) = fence_pen(|chunk| {
            chunk.set_block(10, 64, 8, Block::Dirt);
        });
        assert!(!check(&world, start), "a step beside the fence opens the pen");
    }

    #[test]
    fn swimming_mob_is_not_confined() {
        let world = flat_world(|chunk| {
            chunk.set_water(8, 64, 8, Block::Water, 0);
            chunk.set_water(8, 65, 8, Block::Water, 0);
            chunk.set_water(8, 66, 8, Block::Water, 0);
        });
        // Standing in water with no dry foothold: not "confined", just swimming.
        assert!(!check(&world, IVec3::new(8, 66, 8)));
    }
}
