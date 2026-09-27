use crate::world::ServerWorld;
use rustc_hash::FxHashSet;

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::{section_idx, SectionPos, SECTION_SIZE};
use petramond_world::section::Section;

use crate::world::fluid::fluid_of;

impl ServerWorld {
    /// Kicks fluid once loaded neighbours give it somewhere to go. Reads neighbours by world coord
    /// so it crosses section/column seams, and only flows into neighbours that are actually loaded,
    /// so nothing spills into unstreamed void.
    ///
    /// Also re-arms sim work the streaming-finality guard dropped (`world::sim_guard`): whichever
    /// side of a fluid-air or quench seam lands last re-queues the contact, so nothing gets lost to
    /// gating. Section counters keep bulk cases (calm ocean, deep stone, single-fluid body) cheap.
    /// Cross-cell probes go through a section cursor, walking one section or seam plane at a time,
    /// so most probes hit the cursor's cached section instead of the section map.
    pub(in crate::world) fn queue_loaded_section_fluid_updates(&mut self, ingested: &[SectionPos]) {
        let ingested_set: FxHashSet<SectionPos> = ingested.iter().copied().collect();
        let mut updates: Vec<IVec3> = Vec::new();
        for &sp in ingested {
            let Some(section) = self.data.sections.get(&sp) else {
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
        let cursor = self.data.cursor();
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
                    let open_outflow = || {
                        KICK_OUTFLOW_DIRS.iter().any(|&d| {
                            let n = pos + IVec3::from(d);
                            cursor.section_at(n).is_some()
                                && cursor.chunk_block(n) == Block::Air.id()
                        })
                    };
                    let quenched = || {
                        fluid.quench.is_some_and(|q| {
                            KICK_CONTACT_DIRS.iter().any(|&d| {
                                let n = pos + IVec3::from(d);
                                cursor.chunk_block(n) == q.by.id()
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

    fn kick_outflow_planes(&self, sp: SectionPos, section: &Section, updates: &mut Vec<IVec3>) {
        let cursor = self.data.cursor();
        let blocks = section.blocks();
        for &d in &KICK_OUTFLOW_DIRS {
            let Some(neighbour) = self.data.sections.get(&offset_section(sp, d)) else {
                continue;
            };
            if !neighbour.has_air() {
                continue;
            }
            for_each_seam_cell(sp, d, |local, here, there| {
                if is_fluid(blocks.get(local)) && cursor.chunk_block(there) == Block::Air.id() {
                    updates.push(here);
                }
            });
        }
    }

    fn kick_inflow_planes(&self, sp: SectionPos, section: &Section, updates: &mut Vec<IVec3>) {
        let cursor = self.data.cursor();
        let blocks = section.blocks();
        for &d in &KICK_INFLOW_DIRS {
            let Some(neighbour) = self.data.sections.get(&offset_section(sp, d)) else {
                continue;
            };
            if !neighbour.has_fluid() {
                continue;
            }
            for_each_seam_cell(sp, d, |local, _, there| {
                if blocks.get(local) == Block::Air.id() && is_fluid(cursor.chunk_block(there)) {
                    updates.push(there);
                }
            });
        }
    }

    fn kick_quench_seams(&self, sp: SectionPos, section: &Section, updates: &mut Vec<IVec3>) {
        let cursor = self.data.cursor();
        let blocks = section.blocks();
        for &d in &KICK_CONTACT_DIRS {
            let Some(neighbour) = self.data.sections.get(&offset_section(sp, d)) else {
                continue;
            };
            if !section.may_quench_against(neighbour) {
                continue;
            }
            for_each_seam_cell(sp, d, |local, here, there| {
                let id = blocks.get(local);
                let other = cursor.chunk_block(there);
                if quenched_by(id, other) {
                    updates.push(here);
                }
                if quenched_by(other, id) {
                    updates.push(there);
                }
            });
        }
    }

    /// While this section was gone, the guard dropped checks whose read box touched it (checks up
    /// to `SIM_READ_REACH` cells into loaded neighbours that never unloaded). No scan of the
    /// ingested section catches those, so re-arm every non-source fluid cell in that band.
    /// All-source neighbours (calm ocean) skip via the metadata summary.
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
                        continue;
                    }
                    let Some(neighbour) = self.data.sections.get(&npos) else {
                        continue;
                    };
                    let Some(metas) = neighbour.fluid_slice() else {
                        continue;
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

const KICK_OUTFLOW_DIRS: [Dir; 5] = [(0, -1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];
const KICK_INFLOW_DIRS: [Dir; 5] = [(0, 1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];
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
