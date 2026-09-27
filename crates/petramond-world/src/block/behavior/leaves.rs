use std::collections::VecDeque;

use super::BehaviorWorld;
use crate::mathh::{IVec3, FACE_NEIGHBORS};
use crate::world::data::WorldData;

use super::BlockBehavior;

pub const MAX_LOG_DISTANCE: i32 = 6;

pub struct Leaves;

impl BlockBehavior for Leaves {
    fn key(&self) -> &'static str {
        "leaves"
    }

    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        if !leaf_supported(world.data(), pos) {
            world.break_block_naturally(pos);
        }
    }
}

pub static LEAVES: Leaves = Leaves;

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
                None => return true,
                Some(b) if b.is_log() => return true,
                Some(b) if b.is_leaves() => {
                    let nd = dist + 1;
                    if nd < MAX_LOG_DISTANCE && !visited[offset(n)] {
                        visited[offset(n)] = true;
                        frontier.push_back((n, nd));
                    }
                }
                _ => {}
            }
        }
    }
    false
}

#[cfg(test)]
mod tests;
