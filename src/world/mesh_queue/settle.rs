//! Replica-side mesh settling: a freshly ingested section whose neighbours
//! are still arriving is meshed once they have, not once per arrival.

use petramond_world::chunk::SectionPos;

use crate::world::World;

/// Pump frames a section waits after its latest arrival before meshing with
/// an incomplete neighbourhood — long enough for the rest of a batch to land.
const QUIET_FRAMES: u64 = 2;
/// Pump frames after the FIRST arrival past which the section meshes no
/// matter what keeps arriving, so a busy seam cannot starve it.
const DEADLINE_FRAMES: u64 = 4;
/// Sections this close to the player (in sections, per axis) never wait:
/// their pop-in is what the player is looking at.
const NEAR_RADIUS: i32 = 2;

pub(in crate::world) struct MeshSettle {
    quiet_after: u64,
    deadline: u64,
}

impl World {
    pub(in crate::world) fn defer_stream_mesh(&mut self, pos: SectionPos) {
        if !self.sections.contains_key(&pos) {
            return;
        }
        let frame = self.terrain.mesh_pump_frame;
        let entry = self.terrain.mesh_settle.entry(pos).or_insert(MeshSettle {
            quiet_after: frame + QUIET_FRAMES,
            deadline: frame + DEADLINE_FRAMES,
        });
        entry.quiet_after = frame + QUIET_FRAMES;
    }

    /// Whether `pos` should keep waiting for its neighbourhood. Answers false
    /// (and forgets the entry) once the neighbourhood is complete, the section
    /// is near the player, the quiet window has elapsed, or the deadline hit.
    pub(in crate::world) fn stream_mesh_waiting(&mut self, pos: SectionPos) -> bool {
        let Some(pending) = self.terrain.mesh_settle.get(&pos) else {
            return false;
        };
        let frame = self.terrain.mesh_pump_frame;
        let near = self.last_load_target.is_none_or(|t| {
            (pos.cx - t.center.cx).abs() <= NEAR_RADIUS
                && (pos.cz - t.center.cz).abs() <= NEAR_RADIUS
                && (pos.cy - t.center_cy).abs() <= NEAR_RADIUS
        });
        let complete = (-1..=1).all(|dy| {
            (-1..=1).all(|dz| {
                (-1..=1).all(|dx| {
                    let n = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    !SectionPos::cy_in_range(n.cy)
                        || self.sections.contains_key(&n)
                        || self.section_summary(n)
                            == petramond_world::section::SectionSummary::Empty
                })
            })
        });
        if !near && !complete && frame < pending.quiet_after && frame < pending.deadline {
            return true;
        }
        self.terrain.mesh_settle.remove(&pos);
        false
    }
}

#[cfg(test)]
mod tests;
