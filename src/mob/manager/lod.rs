//! Simulation distance: how much of a mob's tick runs, by how far it is from
//! the nearest player.
//!
//! Mob cost used to scale with everything LOADED (a raised view distance
//! meant a proportionally busier mob tick). The [`SimDistance`] policy
//! scales it with what players can interact with instead:
//! - **full** within [`full_chunks`](SimDistance::full_chunks) of a player:
//!   the whole brain, navigation and physics every tick — unchanged;
//! - **reduced** out to [`reduced_chunks`](SimDistance::reduced_chunks): the
//!   body still moves every tick (physics, route following, pushes), but the
//!   brain decides, and routes are planned, only every
//!   [`reduced_interval`](SimDistance::reduced_interval)th tick — staggered by
//!   mob id so a herd's decisions spread over the interval;
//! - **frozen** beyond: nothing runs but the distance-despawn rule, exactly
//!   like a mob standing over not-yet-loaded terrain.
//!
//! Distances are horizontal chunk distances (Chebyshev, between chunk
//! columns) to the nearest player, and the schedule keys off the world tick
//! and stable ids, so the whole policy is deterministic. The thresholds are a
//! server setting, independent of the view distance.

use serde::{Deserialize, Serialize};

use petramond_math::world_pos::WorldPos;
use petramond_world::chunk::CHUNK_SX;

use super::PlayerAnchor;
use crate::mob::Instance;

/// The simulation-distance thresholds (see the module docs).
/// Unknown fields are ignored, like the rest of the server settings file.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimDistance {
    /// Chunks from the nearest player within which mobs simulate fully.
    pub full_chunks: u32,
    /// Chunks from the nearest player within which mobs simulate at a
    /// reduced AI rate; beyond, they are frozen.
    pub reduced_chunks: u32,
    /// In the reduced band, the brain runs one tick in this many.
    pub reduced_interval: u32,
}

/// The server's default: full simulation well past every perception radius
/// (chase, hearing and the hostile despawn radius all fit inside six
/// chunks), reduced to a quarter-rate brain out to ten.
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
    /// Every mob simulates fully however far it is — the policy of a bare
    /// mob manager (fixtures, tools); servers install their own.
    pub const UNLIMITED: SimDistance = SimDistance {
        full_chunks: u32::MAX,
        reduced_chunks: u32::MAX,
        reduced_interval: 1,
    };

    /// This policy made self-consistent: the reduced band never ends before
    /// the full one, and the interval is at least one tick.
    pub fn sanitized(self) -> SimDistance {
        SimDistance {
            full_chunks: self.full_chunks,
            reduced_chunks: self.reduced_chunks.max(self.full_chunks),
            reduced_interval: self.reduced_interval.max(1),
        }
    }

    /// What `mob` runs on world tick `now`, with `anchors` the players.
    pub(super) fn step(self, mob: &Instance, anchors: &[PlayerAnchor], now: u64) -> SimStep {
        // A corpse finishes its ragdoll and leaves; a mob a mod is driving
        // moves on the mod's schedule, not the policy's.
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

/// What one mob's tick runs this tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SimStep {
    /// The whole tick: brain, planning, physics.
    Think,
    /// Physics and route following on the last decision; no brain, no
    /// planning.
    Coast,
    /// Nothing but the distance-despawn rule.
    Frozen,
}

/// Horizontal Chebyshev distance in chunks from `pos`'s chunk column to the
/// nearest anchor's (`u32::MAX` with no anchors).
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
