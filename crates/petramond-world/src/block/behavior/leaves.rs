//! Leaf decay: a leaf cut off from wood crumbles on random ticks.

use std::collections::VecDeque;

use super::BehaviorWorld;
use crate::mathh::{IVec3, FACE_NEIGHBORS};
use crate::world::data::WorldData;

use super::BlockBehavior;

/// Maximum number of face-steps from a leaf to a log — travelling only through
/// leaves — for the leaf to count as supported.
pub const MAX_LOG_DISTANCE: i32 = 6;

/// Tree leaves. On a random tick a leaf decays to air unless a log is reachable
/// within [`MAX_LOG_DISTANCE`] face-steps travelling only through leaves — so
/// canopy still connected to its trunk persists, while canopy cut off (a felled
/// tree, or a free-standing leaf build) crumbles over the following ticks. A cell
/// in an unloaded chunk met during the search keeps the leaf, so nothing decays on
/// incomplete information at a loaded-area border.
pub struct Leaves;

impl BlockBehavior for Leaves {
    fn key(&self) -> &'static str {
        "leaves"
    }

    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
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
pub fn leaf_supported(world: &WorldData, start: IVec3) -> bool {
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
        for d in FACE_NEIGHBORS {
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

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    pub use super::leaf_supported;
    pub use super::LEAVES;
    pub use super::MAX_LOG_DISTANCE;
    #[allow(unused_imports)]
    pub use super::*;
    pub use crate::mathh::IVec3;
}
