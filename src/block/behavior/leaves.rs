//! Leaf decay: a leaf cut off from wood crumbles on random ticks.

use std::collections::VecDeque;

use crate::mathh::IVec3;
use crate::world::World;

use super::BlockBehavior;

/// Maximum number of face-steps from a leaf to a log — travelling only through
/// leaves — for the leaf to count as supported. (Minecraft tracks a comparable
/// distance-to-wood; here a leaf lives if a log is within 4 steps.)
const MAX_LOG_DISTANCE: i32 = 6;

/// The six face-neighbour offsets, for the leaf-support flood. (The block-update
/// set in `world::tick` is the same six but is private to that module.)
const FACE_OFFSETS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

/// Tree leaves. On a random tick a leaf decays to air unless a log is reachable
/// within [`MAX_LOG_DISTANCE`] face-steps travelling only through leaves — so
/// canopy still connected to its trunk persists, while canopy cut off (a felled
/// tree, or a free-standing leaf build) crumbles over the following ticks. A cell
/// in an unloaded chunk met during the search keeps the leaf, so nothing decays on
/// incomplete information at a loaded-area border.
pub struct Leaves;

impl BlockBehavior for Leaves {
    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut World, pos: IVec3) {
        if !leaf_supported(world, pos) {
            // A leaf cut off from wood crumbles: break it as a natural break so it
            // gets the same burst + rolled drops a hand-break would — for leaves,
            // the 10% chance of a matching sapling (see the leaf rows' `drop`).
            world.break_block_naturally(pos);
        }
    }
}

/// The leaves singleton a row points at (`behavior: &behavior::LEAVES`).
pub static LEAVES: Leaves = Leaves;

/// Whether the leaf at `start` is kept alive: a breadth-first flood through leaf
/// blocks (6-connected) that succeeds the moment it reaches a log within
/// [`MAX_LOG_DISTANCE`] steps. Every cell it can reach lies within that many L1
/// steps of `start`, so `visited` is a fixed `(2·MAX+1)³` stamp addressed by
/// offset — no heap use beyond the small frontier. Meeting an unknown (unloaded /
/// out-of-column) cell returns `true` (keep), so a leaf never decays on missing
/// information.
fn leaf_supported(world: &World, start: IVec3) -> bool {
    const SIDE: usize = (MAX_LOG_DISTANCE * 2 + 1) as usize;
    let mut visited = [false; SIDE * SIDE * SIDE];
    let offset = |p: IVec3| -> usize {
        let ix = (p.x - start.x + MAX_LOG_DISTANCE) as usize;
        let iy = (p.y - start.y + MAX_LOG_DISTANCE) as usize;
        let iz = (p.z - start.z + MAX_LOG_DISTANCE) as usize;
        (iz * SIDE + iy) * SIDE + ix
    };

    visited[offset(start)] = true;
    let mut frontier: VecDeque<(IVec3, i32)> = VecDeque::new();
    frontier.push_back((start, 0));
    while let Some((cell, dist)) = frontier.pop_front() {
        for d in FACE_OFFSETS {
            let n = cell + d;
            match world.block_if_loaded(n.x, n.y, n.z) {
                None => return true,                  // unknown cell: keep the leaf
                Some(b) if b.is_log() => return true, // log at dist + 1 (<= MAX): supported
                Some(b) if b.is_leaves() => {
                    let nd = dist + 1;
                    // Step on only through leaves that can still reach a log in range.
                    if nd < MAX_LOG_DISTANCE && !visited[offset(n)] {
                        visited[offset(n)] = true;
                        frontier.push_back((n, nd));
                    }
                }
                _ => {} // air or any non-wood block: a dead end
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos};

    fn world_with_chunk() -> World {
        let mut w = World::new(1, 1);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    /// Lay a straight +x run of `len` leaves from `start`, then a log — so the log
    /// sits exactly `len` face-steps from `start` through leaves. Stays inside the
    /// 16-wide chunk for `start.x + len <= 15`.
    fn leaf_run_to_log(w: &mut World, start: IVec3, len: i32) {
        for i in 0..len {
            w.set_block_world(start.x + i, start.y, start.z, Block::OakLeaves);
        }
        w.set_block_world(start.x + len, start.y, start.z, Block::OakLog);
    }

    #[test]
    fn log_at_max_distance_supports() {
        let mut w = world_with_chunk();
        let p = IVec3::new(2, 70, 8);
        leaf_run_to_log(&mut w, p, MAX_LOG_DISTANCE);
        assert!(leaf_supported(&w, p));
    }

    #[test]
    fn log_one_step_past_max_does_not_support() {
        let mut w = world_with_chunk();
        let p = IVec3::new(2, 70, 8);
        leaf_run_to_log(&mut w, p, MAX_LOG_DISTANCE + 1);
        assert!(!leaf_supported(&w, p));
    }

    #[test]
    fn adjacent_log_supports() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        w.set_block_world(p.x + 1, p.y, p.z, Block::OakLog);
        assert!(leaf_supported(&w, p));
    }

    #[test]
    fn isolated_leaf_is_unsupported() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        assert!(!leaf_supported(&w, p));
    }

    #[test]
    fn a_decaying_leaf_breaks_naturally_so_its_drop_can_roll() {
        // A leaf cut off from wood doesn't vanish silently: it breaks as a NATURAL
        // break, so `Game` plays the burst and rolls the leaf's drop table — the 10%
        // sapling. Here we assert the decay is recorded as a natural break (the drop
        // hand-off), independent of the probabilistic roll itself.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::OakLeaves); // isolated → unsupported
        LEAVES.random_tick(&mut w, p);
        assert_eq!(
            w.block_if_loaded(p.x, p.y, p.z),
            Some(Block::Air),
            "the leaf decayed"
        );
        let breaks = w.take_natural_breaks();
        assert!(
            breaks
                .iter()
                .any(|&(bp, b)| bp == p && b == Block::OakLeaves),
            "a decayed leaf is recorded as a natural break so its sapling drop rolls",
        );
    }
}
