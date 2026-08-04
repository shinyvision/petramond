use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{section_idx, SectionPos, SECTION_SIZE};
use crate::light::LightRgb;
use crate::mathh::IVec3;
use crate::section::Section;

use super::shape::SparseCellState;
use super::{nbhd_idx, NBHD_VOLUME};

/// Shared block buffers of a section's 3x3x3 neighbourhood, indexed by [`arc_idx`].
/// `None` for an absent neighbour, which reads as air.
type BlockArcs = [Option<crate::section::BlockCube>; 27];

pub struct Snapshot {
    blocks: BlockArcs,
    states: Vec<SparseCellState>,
}

impl Snapshot {
    pub fn states(&self) -> &[SparseCellState] {
        &self.states
    }
}

#[inline]
fn arc_idx(dcx: i32, dcy: i32, dcz: i32) -> usize {
    (((dcy + 1) * 3 + (dcz + 1)) * 3 + (dcx + 1)) as usize
}

/// Take cheap shared handles plus sparse per-cell light state for `pos`'s 3x3x3
/// neighbourhood. Runs on the main thread; dense buffers are assembled in the worker.
pub fn gather(pos: SectionPos, sections: &FxHashMap<SectionPos, Arc<Section>>) -> Snapshot {
    let mut blocks: BlockArcs = std::array::from_fn(|_| None);
    let mut states = Vec::new();
    for dcy in -1..=1 {
        for dcz in -1..=1 {
            for dcx in -1..=1 {
                let npos = SectionPos::new(pos.cx + dcx, pos.cy + dcy, pos.cz + dcz);
                let Some(section) = sections.get(&npos) else {
                    continue;
                };
                blocks[arc_idx(dcx, dcy, dcz)] = Some(section.block_cube());
                let bx = ((dcx + 1) as usize) * SECTION_SIZE;
                let by = ((dcy + 1) as usize) * SECTION_SIZE;
                let bz = ((dcz + 1) as usize) * SECTION_SIZE;
                super::shape::collect_shape_states(
                    section,
                    |lx, ly, lz| nbhd_idx(bx + lx, by + ly, bz + lz),
                    &mut states,
                );
                if let Some(aps) = section.custom_light_apertures() {
                    // A WASM shape's baked per-cell opacity, in the same
                    // aperture currency the families answer: opaque blocks
                    // every quadrant, open passes all.
                    states.extend(aps.iter().map(|(&key, &opaque)| {
                        let (lx, ly, lz) = crate::chunk::section_local(key as usize);
                        SparseCellState {
                            idx: nbhd_idx(bx + lx, by + ly, bz + lz),
                            masks: if opaque {
                                0
                            } else {
                                crate::block::LIGHT_APERTURES_OPEN
                            },
                        }
                    }));
                }
            }
        }
    }
    Snapshot { blocks, states }
}

/// Assemble the neighbourhood block-id cube into `out` (a reused per-thread
/// buffer of `NBHD_VOLUME` bytes). Absent neighbours read as air.
pub fn assemble_blocks(snapshot: &Snapshot, out: &mut [u16]) {
    debug_assert_eq!(out.len(), NBHD_VOLUME);
    out.fill(0);
    for dcy in -1..=1 {
        for dcz in -1..=1 {
            for dcx in -1..=1 {
                let Some(src) = &snapshot.blocks[arc_idx(dcx, dcy, dcz)] else {
                    continue;
                };
                let bx = ((dcx + 1) as usize) * SECTION_SIZE;
                let by = ((dcy + 1) as usize) * SECTION_SIZE;
                let bz = ((dcz + 1) as usize) * SECTION_SIZE;
                // Both layouts run X fastest, so a section row is one copy.
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        let d = nbhd_idx(bx, by + ly, bz + lz);
                        let s = section_idx(0, ly, lz);
                        src.expand_row_into(s, &mut out[d..d + SECTION_SIZE]);
                    }
                }
            }
        }
    }
}

/// Collect every block-light emitter in `pos`'s 3x3x3 section neighbourhood,
/// as `(cell, emitted colour)` seeds for the flood.
pub fn collect_emitters(
    pos: SectionPos,
    sections: &FxHashMap<SectionPos, Arc<Section>>,
) -> Vec<(IVec3, LightRgb)> {
    let mut emitters = Vec::new();
    for dcy in -1..=1 {
        for dcz in -1..=1 {
            for dcx in -1..=1 {
                let npos = SectionPos::new(pos.cx + dcx, pos.cy + dcy, pos.cz + dcz);
                if let Some(section) = sections.get(&npos) {
                    collect_section_emitters(npos, section, &mut emitters);
                }
            }
        }
    }
    emitters
}

/// Emitters are pure block-row data: any cell whose block declares
/// `emission > 0` seeds the flood with that row's COLOUR (torches, the LIT
/// furnace row, pack glow blocks) — no per-block-kind state map is consulted.
/// The per-section `light_emitter_count` gate keeps this scan off the (vastly
/// common) emitter-free sections, and both per-cell reads go through dense
/// per-id tables — the scalar `emission` is the gate (one byte, taken 4096
/// times) and the RGB triple is fetched only on the rare hit.
pub fn collect_section_emitters(
    pos: SectionPos,
    section: &Section,
    out: &mut Vec<(IVec3, LightRgb)>,
) {
    if !section.has_light_emitters() {
        return;
    }
    let (ox, oy, oz) = pos.origin_world();
    for (idx, id) in section.blocks_iter().enumerate() {
        let block = crate::block::Block::from_id(id);
        if block.light_emission() > 0 {
            let [r, g, b] = block.light_emission_rgb();
            let (lx, ly, lz) = crate::chunk::section_local(idx);
            out.push((
                IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32),
                LightRgb::new(r, g, b),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;

    /// The seed must carry the row's whole COLOUR, not its brightness. Seeding
    /// `grey(emission)` would still light the cave correctly and pass every
    /// intensity assertion — and silently delete the feature.
    #[test]
    fn an_emitter_seeds_the_flood_with_its_rows_colour() {
        let pos = SectionPos::new(2, -1, 4);
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.set_block(3, 5, 7, Block::Torch);

        let mut out = Vec::new();
        collect_section_emitters(pos, &section, &mut out);

        let (ox, oy, oz) = pos.origin_world();
        let [r, g, b] = Block::Torch.light_emission_rgb();
        assert_eq!(
            out,
            vec![(IVec3::new(ox + 3, oy + 5, oz + 7), LightRgb::new(r, g, b))]
        );
    }
}
