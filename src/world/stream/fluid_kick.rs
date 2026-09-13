use rustc_hash::FxHashSet;

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::{section_idx, SectionPos, SECTION_SIZE};
use petramond_world::section::Section;

use crate::world::fluid::fluid_of;
use crate::world::store::World;

impl World {
    /// Kick generated/overlaid fluid once its loaded neighbourhood gives it
    /// somewhere to go. Reads neighbours by world coordinate (so it crosses
    /// section and column seams) and only flows into a neighbour that is
    /// actually loaded, so fluid never spills into a not-yet-streamed void.
    ///
    /// The kick is also the RE-ARM for simulation work the streaming-finality
    /// guard dropped (`world::sim_guard`): whichever side of a fluid-air or
    /// quench seam lands LAST re-queues the contact, so no flow is permanently
    /// lost to gating. Each step is cheap in the bulk cases (calm ocean, deep
    /// stone, a single-fluid body) by the section counters.
    pub(in crate::world) fn queue_loaded_section_fluid_updates(&mut self, ingested: &[SectionPos]) {
        let ingested_set: FxHashSet<SectionPos> = ingested.iter().copied().collect();
        let mut updates: Vec<IVec3> = Vec::new();
        for &sp in ingested {
            let Some(section) = self.sections.get(&sp) else {
                continue;
            };
            if section.has_fluid() {
                if section.has_air()
                    || section.has_flowing_fluid()
                    || section.may_quench_against(section)
                {
                    self.kick_interior(sp, section, &mut updates);
                } else {
                    self.kick_outflow_planes(sp, section, &mut updates);
                }
                self.kick_quench_seams(sp, section, &mut updates);
            }
            if section.has_air() {
                self.kick_inflow_planes(sp, section, &mut updates);
            }
            self.rearm_dropped_neighbour_checks(sp, &ingested_set, &mut updates);
        }
        for pos in updates {
            self.queue_block_update(pos);
        }
    }

    /// Fluid with air, flowing cells or a possible quench inside the section:
    /// scan every fluid cell. A non-source cell always re-arms — a flow check
    /// also re-levels and DRIES, and pending checks died with the unload, so an
    /// enclosed mid-drain sheet would otherwise freeze at flowing levels
    /// forever; a settled cell recomputes to itself and writes nothing. A source
    /// re-arms only next to loaded air (spread is all it does) or when it
    /// touches the fluid that quenches it, so a generated contact is judged
    /// instead of sitting unjudged until disturbed.
    fn kick_interior(&self, sp: SectionPos, section: &Section, updates: &mut Vec<IVec3>) {
        let (ox, oy, oz) = sp.origin_world();
        let blocks = section.blocks();
        let metas = section.fluid_slice();
        for ly in 0..SECTION_SIZE {
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    let idx = section_idx(lx, ly, lz);
                    let Some(fluid) = fluid_of(Block::from_id(blocks.get(idx))) else {
                        continue;
                    };
                    let pos = IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32);
                    let flowing = metas.is_some_and(|m| m[idx] != 0);
                    // Air above is a normal surface and does not start flow.
                    let open_outflow = || {
                        KICK_OUTFLOW_DIRS.iter().any(|&d| {
                            let n = pos + IVec3::from(d);
                            self.section_loaded_at(n.x, n.y, n.z)
                                && self.chunk_block(n.x, n.y, n.z) == Block::Air.id()
                        })
                    };
                    // An unloaded neighbour reads as air, never as the quencher.
                    let quenched = || {
                        fluid.quench.is_some_and(|q| {
                            KICK_CONTACT_DIRS.iter().any(|&d| {
                                let n = pos + IVec3::from(d);
                                self.chunk_block(n.x, n.y, n.z) == q.by.id()
                            })
                        })
                    };
                    if flowing || open_outflow() || quenched() {
                        updates.push(pos);
                    }
                }
            }
        }
    }

    /// All-source fluid without air (ocean interior, fluid over a sealed
    /// floor): only boundary fluid can flow, outward through the five outflow
    /// planes, and only against a loaded neighbour that holds air.
    fn kick_outflow_planes(&self, sp: SectionPos, section: &Section, updates: &mut Vec<IVec3>) {
        let blocks = section.blocks();
        for &d in &KICK_OUTFLOW_DIRS {
            let Some(neighbour) = self.sections.get(&offset_section(sp, d)) else {
                continue; // absent: its own landing kick handles the seam
            };
            if !neighbour.has_air() {
                continue; // full fluid/stone plane cannot accept flow
            }
            for_each_seam_cell(sp, d, |local, here, there| {
                if is_fluid(blocks.get(local))
                    && self.chunk_block(there.x, there.y, there.z) == Block::Air.id()
                {
                    updates.push(here);
                }
            });
        }
    }

    /// Any air: fluid in a LOADED neighbour may now flow into it, from above
    /// (falling in) or from the sides — the cross-seam case neither section's
    /// own fluid scan can see. Queues the NEIGHBOUR's fluid cell.
    fn kick_inflow_planes(&self, sp: SectionPos, section: &Section, updates: &mut Vec<IVec3>) {
        let blocks = section.blocks();
        for &d in &KICK_INFLOW_DIRS {
            let Some(neighbour) = self.sections.get(&offset_section(sp, d)) else {
                continue;
            };
            if !neighbour.has_fluid() {
                continue;
            }
            for_each_seam_cell(sp, d, |local, _, there| {
                if blocks.get(local) == Block::Air.id()
                    && is_fluid(self.chunk_block(there.x, there.y, there.z))
                {
                    updates.push(there);
                }
            });
        }
    }

    /// A quench contact lying exactly on a section seam, queued on whichever
    /// side holds the quenching fluid — so it re-arms in either landing order.
    /// Kept six-directional: fluid below still has to queue the reacting cell's
    /// downward pour.
    fn kick_quench_seams(&self, sp: SectionPos, section: &Section, updates: &mut Vec<IVec3>) {
        let blocks = section.blocks();
        for &d in &KICK_CONTACT_DIRS {
            let Some(neighbour) = self.sections.get(&offset_section(sp, d)) else {
                continue;
            };
            if !section.may_quench_against(neighbour) {
                continue;
            }
            for_each_seam_cell(sp, d, |local, here, there| {
                let id = blocks.get(local);
                let other = self.chunk_block(there.x, there.y, there.z);
                if quenched_by(id, other) {
                    updates.push(here);
                }
                if quenched_by(other, id) {
                    updates.push(there);
                }
            });
        }
    }

    /// While this section was absent the guard DROPPED fired checks whose read
    /// box touched it — checks living up to `SIM_READ_REACH` cells inside loaded
    /// neighbours that never unloaded, which no scan of the ingested section
    /// sees. Re-arm every non-source fluid cell in that band; all-source
    /// neighbours (calm ocean) skip by the metadata summary.
    fn rearm_dropped_neighbour_checks(
        &self,
        sp: SectionPos,
        ingested: &FxHashSet<SectionPos>,
        updates: &mut Vec<IVec3>,
    ) {
        const REACH: usize = crate::world::sim_guard::SIM_READ_REACH as usize;
        let band = |d: i32| match d {
            -1 => SECTION_SIZE - REACH..SECTION_SIZE,
            1 => 0..REACH,
            _ => 0..SECTION_SIZE,
        };
        for dy in -1..=1i32 {
            for dz in -1..=1i32 {
                for dx in -1..=1i32 {
                    if (dx, dy, dz) == (0, 0, 0) {
                        continue;
                    }
                    let npos = offset_section(sp, (dx, dy, dz));
                    if ingested.contains(&npos) {
                        continue; // its own interior scan covers it fully
                    }
                    let Some(neighbour) = self.sections.get(&npos) else {
                        continue;
                    };
                    let Some(metas) = neighbour.fluid_slice() else {
                        continue; // no mid-flow cells: nothing to re-arm
                    };
                    let blocks = neighbour.blocks();
                    let (ox, oy, oz) = npos.origin_world();
                    for ly in band(dy) {
                        for lz in band(dz) {
                            for lx in band(dx) {
                                let idx = section_idx(lx, ly, lz);
                                if metas[idx] != 0 && is_fluid(blocks.get(idx)) {
                                    updates.push(IVec3::new(
                                        ox + lx as i32,
                                        oy + ly as i32,
                                        oz + lz as i32,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

type Dir = (i32, i32, i32);

/// Fluid can leave a section down or sideways (never up).
const KICK_OUTFLOW_DIRS: [Dir; 5] = [(0, -1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];
/// Fluid can enter a section's air from above (falling) or from the sides
/// (never rising from below).
const KICK_INFLOW_DIRS: [Dir; 5] = [(0, 1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];
/// A quench contact is judged over the full 6-neighbourhood.
const KICK_CONTACT_DIRS: [Dir; 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (-1, 0, 0),
    (1, 0, 0),
    (0, 0, -1),
    (0, 0, 1),
];

#[inline]
fn is_fluid(id: u16) -> bool {
    fluid_of(Block::from_id(id)).is_some()
}

/// Whether a cell of `id` is quenched by a neighbouring cell of `other`.
#[inline]
fn quenched_by(id: u16, other: u16) -> bool {
    fluid_of(Block::from_id(id))
        .and_then(|f| f.quench)
        .is_some_and(|q| q.by.id() == other)
}

#[inline]
fn offset_section(sp: SectionPos, (dx, dy, dz): Dir) -> SectionPos {
    SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz)
}

/// Visit the 16×16 boundary plane of `sp` facing `d`: the section-local index
/// of each cell, its world position, and the world position across the seam.
fn for_each_seam_cell(sp: SectionPos, d: Dir, mut visit: impl FnMut(usize, IVec3, IVec3)) {
    let (ox, oy, oz) = sp.origin_world();
    let step = IVec3::from(d);
    for a in 0..SECTION_SIZE {
        for b in 0..SECTION_SIZE {
            let (lx, ly, lz) = boundary_cell(d, a, b);
            let here = IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32);
            visit(section_idx(lx, ly, lz), here, here + step);
        }
    }
}

/// The section-local cell on the boundary plane facing `d`, indexed by the
/// plane's two free axes `(a, b)`.
#[inline]
fn boundary_cell(d: Dir, a: usize, b: usize) -> (usize, usize, usize) {
    let hi = SECTION_SIZE - 1;
    match d {
        (1, 0, 0) => (hi, a, b),
        (-1, 0, 0) => (0, a, b),
        (0, 1, 0) => (a, hi, b),
        (0, -1, 0) => (a, 0, b),
        (0, 0, 1) => (a, b, hi),
        _ => (a, b, 0),
    }
}
