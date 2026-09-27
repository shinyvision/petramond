use serde::{Deserialize, Serialize};

use petramond_math::world_pos::WorldPos;
use petramond_world::chunk::CHUNK_SX;

use super::PlayerAnchor;
use crate::mob::Instance;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimDistance {
    pub full_chunks: u32,
    pub reduced_chunks: u32,
    pub reduced_interval: u32,
}

impl Default for SimDistance {
    fn default() -> Self {
        SimDistance {
            full_chunks: 6,
            reduced_chunks: 10,
            reduced_interval: 4,
        }
    }
}

impl SimDistance {
    pub const UNLIMITED: SimDistance = SimDistance {
        full_chunks: u32::MAX,
        reduced_chunks: u32::MAX,
        reduced_interval: 1,
    };

    pub fn sanitized(self) -> SimDistance {
        SimDistance {
            full_chunks: self.full_chunks,
            reduced_chunks: self.reduced_chunks.max(self.full_chunks),
            reduced_interval: self.reduced_interval.max(1),
        }
    }

    pub(super) fn step(self, mob: &Instance, anchors: &[PlayerAnchor], now: u64) -> SimStep {
        if mob.is_dead() || mob.externally_driven() {
            return SimStep::Think;
        }
        let distance = chunk_distance(anchors, mob.pos);
        if distance <= self.full_chunks {
            SimStep::Think
        } else if distance <= self.reduced_chunks {
            let interval = u64::from(self.reduced_interval.max(1));
            if now.wrapping_add(mob.id()).is_multiple_of(interval) {
                SimStep::Think
            } else {
                SimStep::Coast
            }
        } else {
            SimStep::Frozen
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SimStep {
    Think,
    Coast,
    Frozen,
}

pub(super) fn chunk_distance(anchors: &[PlayerAnchor], pos: WorldPos) -> u32 {
    let column = |p: WorldPos| {
        let c = p.block();
        (
            c.x.div_euclid(CHUNK_SX as i32),
            c.z.div_euclid(CHUNK_SX as i32),
        )
    };
    let (mx, mz) = column(pos);
    anchors
        .iter()
        .map(|a| {
            let (ax, az) = column(a.pos);
            mx.abs_diff(ax).max(mz.abs_diff(az))
        })
        .min()
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
