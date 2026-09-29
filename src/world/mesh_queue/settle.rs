use std::time::{Duration, Instant};

use crate::world::ReplicaWorld;
use petramond_world::chunk::SectionPos;

/// How long a section waits after the latest arrival in its neighbourhood
/// before meshing an incomplete one. Streaming lands a section's outward
/// neighbours about one ring after it, so a window shorter than that meshes
/// nearly every section twice — and a section that turns out sealed or
/// hidden once they land never needed meshing at all.
const QUIET: Duration = Duration::from_millis(500);
const DEADLINE: Duration = Duration::from_millis(1000);
const NEAR_RADIUS: i32 = 2;

pub(in crate::world) struct MeshSettle {
    quiet_after: Instant,
    deadline: Instant,
}

impl ReplicaWorld {
    pub(in crate::world) fn defer_stream_mesh(&mut self, pos: SectionPos) {
        if !self.data.sections.contains_key(&pos) {
            return;
        }
        let now = self.side.terrain.mesh_pump_now;
        let entry = self
            .side
            .terrain
            .mesh_settle
            .entry(pos)
            .or_insert(MeshSettle {
                quiet_after: now + QUIET,
                deadline: now + DEADLINE,
            });
        entry.quiet_after = now + QUIET;
    }

    #[cfg(test)]
    pub(in crate::world) fn stream_mesh_waiting(&mut self, pos: SectionPos) -> bool {
        let nbhd = self.gather_mesh_neighbourhood(pos);
        self.stream_mesh_waiting_in(pos, &nbhd)
    }

    pub(in crate::world) fn stream_mesh_waiting_in(
        &mut self,
        pos: SectionPos,
        nbhd: &super::mesh_jobs::MeshNbhd,
    ) -> bool {
        let Some(pending) = self.side.terrain.mesh_settle.get(&pos) else {
            return false;
        };
        let now = self.side.terrain.mesh_pump_now;
        let near = self.data.last_load_target.is_none_or(|t| {
            (pos.cx - t.center.cx).abs() <= NEAR_RADIUS
                && (pos.cz - t.center.cz).abs() <= NEAR_RADIUS
                && (pos.cy - t.center_cy).abs() <= NEAR_RADIUS
        });
        // The 27-probe completeness walk only decides anything while the window is still open.
        let complete = || {
            (-1..=1).all(|dy| {
                (-1..=1).all(|dz| {
                    (-1..=1).all(|dx| {
                        let n = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                        !SectionPos::cy_in_range(n.cy)
                            || nbhd[crate::world::mesh_pool::nbhd_idx27(dx, dy, dz)].is_some()
                            || self.data.section_summary(n)
                                == petramond_world::section::SectionSummary::Empty
                    })
                })
            })
        };
        if !near && now < pending.quiet_after && now < pending.deadline && !complete() {
            return true;
        }
        self.side.terrain.mesh_settle.remove(&pos);
        false
    }
}

#[cfg(test)]
mod tests;
