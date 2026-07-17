use rustc_hash::FxHashSet;

use crate::block::Block;
use crate::chunk::{section_idx, SectionPos, SECTION_SIZE};
use crate::mathh::IVec3;

use crate::world::store::World;

impl World {
    /// Kick generated/overlaid source water into flowing once its loaded neighbourhood
    /// gives it somewhere to go: down into air, or sideways into air. Reads neighbours by
    /// world coordinate (so it crosses section and column seams) and only flows into a
    /// neighbour that is actually loaded, so water never spills into a not-yet-streamed
    /// void.
    ///
    /// The kick is also the RE-ARM for simulation work the streaming-finality guard
    /// dropped (`world::sim_guard`): whichever side of a water-air seam lands LAST
    /// re-queues the contact, so no flow is permanently lost to gating. Four scans
    /// per ingested section, each cheap in the bulk cases:
    /// - water with air or FLOWING cells inside the section: the full interior scan
    ///   (shores, waterfalls, reloaded mid-flow water). A non-source cell always
    ///   re-arms — a flow check also re-levels and DRIES, and pending checks died
    ///   with the unload, so an enclosed mid-drain sheet would otherwise freeze at
    ///   flowing levels forever; a settled cell recomputes to itself and writes
    ///   nothing, so the kick stays cheap. Sources re-arm only next to loaded air
    ///   (spread is all they do);
    /// - all-source water without air (ocean interior, water over a sealed floor):
    ///   only the five outflow boundary planes, and only against a loaded neighbour
    ///   that holds air — calm open ocean skips every plane by summary;
    /// - any air: the five inflow boundary planes against loaded water-holding
    ///   neighbours, queueing the NEIGHBOUR's water cell — the cross-seam case
    ///   neither section's own water scan can see (its water, this section's air);
    /// - non-source water within `SIM_READ_REACH` of this section in every loaded
    ///   neighbour: while this section was absent, the guard DROPPED fired checks
    ///   whose read box touched it — checks living up to `SIM_READ_REACH` cells
    ///   inside sections that never unloaded, which no ingested-section scan sees.
    ///   All-source neighbours (calm ocean) skip by the metadata summary.
    pub(in crate::world) fn queue_loaded_section_water_updates(&mut self, ingested: &[SectionPos]) {
        const REACH: usize = crate::world::sim_guard::SIM_READ_REACH as usize;
        let air = Block::Air.id();
        let water = Block::Water.id();
        let ingested_set: FxHashSet<SectionPos> = ingested.iter().copied().collect();
        let mut updates: Vec<IVec3> = Vec::new();
        for sp in ingested {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            let (ox, oy, oz) = sp.origin_world();
            let has_water = section.has_water();
            let has_air = section.has_air();
            let has_flowing = section
                .water_slice()
                .is_some_and(|metas| metas.iter().any(|&m| m != 0));

            if has_water && (has_air || has_flowing) {
                let blocks = section.blocks_slice();
                let metas = section.water_slice();
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            let idx = section_idx(lx, ly, lz);
                            if blocks[idx] != water {
                                continue;
                            }
                            let wx = ox + lx as i32;
                            let wy = oy + ly as i32;
                            let wz = oz + lz as i32;
                            if metas.is_some_and(|m| m[idx] != 0) {
                                updates.push(IVec3::new(wx, wy, wz));
                                continue;
                            }
                            // Down + the four horizontals (air above is a normal
                            // surface and does not start flow).
                            let neighbors = [
                                (wx, wy - 1, wz),
                                (wx - 1, wy, wz),
                                (wx + 1, wy, wz),
                                (wx, wy, wz - 1),
                                (wx, wy, wz + 1),
                            ];
                            if neighbors.iter().any(|&(nx, ny, nz)| {
                                self.section_loaded_at(nx, ny, nz)
                                    && self.chunk_block(nx, ny, nz) == air
                            }) {
                                updates.push(IVec3::new(wx, wy, wz));
                            }
                        }
                    }
                }
            } else if has_water {
                // No air inside: only boundary water can flow, and only outward
                // through the five outflow faces.
                let blocks = section.blocks_slice();
                for &(dx, dy, dz) in &KICK_OUTFLOW_DIRS {
                    let npos = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                    let Some(ns) = self.sections.get(&npos) else {
                        continue; // absent: its own landing kick handles the seam
                    };
                    if !ns.has_air() {
                        continue; // full water/stone plane cannot accept flow
                    }
                    for a in 0..SECTION_SIZE {
                        for b in 0..SECTION_SIZE {
                            let (lx, ly, lz) = boundary_cell(dx, dy, dz, a, b);
                            if blocks[section_idx(lx, ly, lz)] != water {
                                continue;
                            }
                            let (wx, wy, wz) = (
                                ox + lx as i32 + dx,
                                oy + ly as i32 + dy,
                                oz + lz as i32 + dz,
                            );
                            if self.chunk_block(wx, wy, wz) == air {
                                updates.push(IVec3::new(wx - dx, wy - dy, wz - dz));
                            }
                        }
                    }
                }
            }

            if has_air {
                // Water in a LOADED neighbour may now have this section's air to
                // flow into: from above (falls in) or from the four sides.
                let blocks = section.blocks_slice();
                for &(dx, dy, dz) in &KICK_INFLOW_DIRS {
                    let npos = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                    let Some(ns) = self.sections.get(&npos) else {
                        continue;
                    };
                    if !ns.has_water() {
                        continue;
                    }
                    for a in 0..SECTION_SIZE {
                        for b in 0..SECTION_SIZE {
                            let (lx, ly, lz) = boundary_cell(dx, dy, dz, a, b);
                            if blocks[section_idx(lx, ly, lz)] != air {
                                continue;
                            }
                            let (nx, ny, nz) = (
                                ox + lx as i32 + dx,
                                oy + ly as i32 + dy,
                                oz + lz as i32 + dz,
                            );
                            if self.chunk_block(nx, ny, nz) == water {
                                updates.push(IVec3::new(nx, ny, nz));
                            }
                        }
                    }
                }
            }

            // Guard-drop re-arm (doc above): non-source water in the loaded,
            // non-ingested 26-neighbourhood within read reach of this section.
            let local_range = |d: i32| match d {
                -1 => SECTION_SIZE - REACH..SECTION_SIZE,
                1 => 0..REACH,
                _ => 0..SECTION_SIZE,
            };
            for ndy in -1..=1i32 {
                for ndz in -1..=1i32 {
                    for ndx in -1..=1i32 {
                        if ndx == 0 && ndy == 0 && ndz == 0 {
                            continue;
                        }
                        let npos = SectionPos::new(sp.cx + ndx, sp.cy + ndy, sp.cz + ndz);
                        if ingested_set.contains(&npos) {
                            continue; // its own interior scan covers it fully
                        }
                        let Some(ns) = self.sections.get(&npos) else {
                            continue;
                        };
                        let Some(metas) = ns.water_slice() else {
                            continue; // all sources: nothing mid-flow to re-arm
                        };
                        let nblocks = ns.blocks_slice();
                        let (nox, noy, noz) = npos.origin_world();
                        for ly in local_range(ndy) {
                            for lz in local_range(ndz) {
                                for lx in local_range(ndx) {
                                    let idx = section_idx(lx, ly, lz);
                                    if metas[idx] != 0 && nblocks[idx] == water {
                                        updates.push(IVec3::new(
                                            nox + lx as i32,
                                            noy + ly as i32,
                                            noz + lz as i32,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for pos in updates {
            self.queue_block_update(pos);
        }
    }
}

/// Water can leave a section down or sideways (never up).
const KICK_OUTFLOW_DIRS: [(i32, i32, i32); 5] =
    [(0, -1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];
/// Water can enter a section's air from above (falling) or from the sides
/// (never rising from below).
const KICK_INFLOW_DIRS: [(i32, i32, i32); 5] =
    [(0, 1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];

/// The section-local cell on the boundary plane facing `(dx,dy,dz)`, indexed by
/// the plane's two free axes `(a, b)`.
#[inline]
fn boundary_cell(dx: i32, dy: i32, dz: i32, a: usize, b: usize) -> (usize, usize, usize) {
    let hi = SECTION_SIZE - 1;
    match (dx, dy, dz) {
        (1, 0, 0) => (hi, a, b),
        (-1, 0, 0) => (0, a, b),
        (0, 1, 0) => (a, hi, b),
        (0, -1, 0) => (a, 0, b),
        (0, 0, 1) => (a, b, hi),
        _ => (a, b, 0),
    }
}
