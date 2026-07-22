use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{section_idx, SectionPos, SECTION_SIZE};
use crate::mathh::IVec3;
use crate::section::Section;

use super::shape::SparseCellState;
use super::{nbhd_idx, NBHD_VOLUME};

/// Shared block buffers of a section's 3x3x3 neighbourhood, indexed by [`arc_idx`].
/// `None` for an absent neighbour, which reads as air.
type BlockArcs = [Option<Arc<[u8]>>; 27];

pub(super) struct Snapshot {
    blocks: BlockArcs,
    states: Vec<SparseCellState>,
}

impl Snapshot {
    pub(super) fn states(&self) -> &[SparseCellState] {
        &self.states
    }
}

#[inline]
fn arc_idx(dcx: i32, dcy: i32, dcz: i32) -> usize {
    (((dcy + 1) * 3 + (dcz + 1)) * 3 + (dcx + 1)) as usize
}

/// Take cheap shared handles plus sparse per-cell light state for `pos`'s 3x3x3
/// neighbourhood. Runs on the main thread; dense buffers are assembled in the worker.
pub(super) fn gather(pos: SectionPos, sections: &FxHashMap<SectionPos, Arc<Section>>) -> Snapshot {
    let mut blocks: BlockArcs = std::array::from_fn(|_| None);
    let mut states = Vec::new();
    for dcy in -1..=1 {
        for dcz in -1..=1 {
            for dcx in -1..=1 {
                let npos = SectionPos::new(pos.cx + dcx, pos.cy + dcy, pos.cz + dcz);
                let Some(section) = sections.get(&npos) else {
                    continue;
                };
                blocks[arc_idx(dcx, dcy, dcz)] = Some(section.blocks_arc());
                let bx = ((dcx + 1) as usize) * SECTION_SIZE;
                let by = ((dcy + 1) as usize) * SECTION_SIZE;
                let bz = ((dcz + 1) as usize) * SECTION_SIZE;
                states.extend(section.stair_states().iter().map(|(&key, &state)| {
                    let (lx, ly, lz) = crate::chunk::section_local(key as usize);
                    SparseCellState::Stair {
                        idx: nbhd_idx(bx + lx, by + ly, bz + lz),
                        state,
                    }
                }));
                states.extend(section.slab_states().iter().map(|(&key, &state)| {
                    let (lx, ly, lz) = crate::chunk::section_local(key as usize);
                    SparseCellState::Slab {
                        idx: nbhd_idx(bx + lx, by + ly, bz + lz),
                        state,
                    }
                }));
                if let Some(aps) = section.custom_light_apertures() {
                    states.extend(aps.iter().map(|(&key, &opaque)| {
                        let (lx, ly, lz) = crate::chunk::section_local(key as usize);
                        SparseCellState::CustomAperture {
                            idx: nbhd_idx(bx + lx, by + ly, bz + lz),
                            opaque,
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
pub(super) fn assemble_blocks(snapshot: &Snapshot, out: &mut [u8]) {
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
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            out[nbhd_idx(bx + lx, by + ly, bz + lz)] = src[section_idx(lx, ly, lz)];
                        }
                    }
                }
            }
        }
    }
}

/// Collect every block-light emitter in `pos`'s 3x3x3 section neighbourhood,
/// as `(cell, emission)` seeds for the flood.
pub(super) fn collect_emitters(
    pos: SectionPos,
    sections: &FxHashMap<SectionPos, Arc<Section>>,
) -> Vec<(IVec3, u8)> {
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
/// `emission > 0` seeds the flood at that level (torches, the LIT furnace row,
/// pack glow blocks) — no per-block-kind state map is consulted. The
/// per-section `light_emitter_count` gate keeps this scan off the (vastly
/// common) emitter-free sections, and the per-cell read goes through the dense
/// per-id emission table.
pub(super) fn collect_section_emitters(
    pos: SectionPos,
    section: &Section,
    out: &mut Vec<(IVec3, u8)>,
) {
    if !section.has_light_emitters() {
        return;
    }
    let (ox, oy, oz) = pos.origin_world();
    for (idx, &id) in section.blocks_slice().iter().enumerate() {
        let emission = crate::block::Block::from_id(id).light_emission();
        if emission > 0 {
            let (lx, ly, lz) = crate::chunk::section_local(idx);
            out.push((
                IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32),
                emission,
            ));
        }
    }
}
