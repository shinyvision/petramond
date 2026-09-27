use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{section_idx, SectionPos, SECTION_SIZE};
use crate::light::LightRgb;
use crate::mathh::IVec3;
use crate::section::{BlockCube, Section};

use super::shape::{ShapeStateSnapshot, SparseCellState};

#[inline]
pub fn cube_idx(dim: usize, x: usize, y: usize, z: usize) -> usize {
    (y * dim + z) * dim + x
}

pub struct Snapshot {
    span: usize,
    blocks: Vec<Option<BlockCube>>,
    states: Vec<SparseCellState>,
}

#[inline]
fn span_idx(span: usize, dx: usize, dy: usize, dz: usize) -> usize {
    (dy * span + dz) * span + dx
}

#[inline]
fn window_pos(low: SectionPos, dx: usize, dy: usize, dz: usize) -> SectionPos {
    SectionPos::new(low.cx + dx as i32, low.cy + dy as i32, low.cz + dz as i32)
}

impl Snapshot {
    pub fn gather(
        low: SectionPos,
        span: usize,
        sections: &FxHashMap<SectionPos, Arc<Section>>,
    ) -> Self {
        let dim = span * SECTION_SIZE;
        let mut blocks = vec![None; span * span * span];
        let mut states = Vec::new();
        for dy in 0..span {
            for dz in 0..span {
                for dx in 0..span {
                    let Some(section) = sections.get(&window_pos(low, dx, dy, dz)) else {
                        continue;
                    };
                    blocks[span_idx(span, dx, dy, dz)] = Some(section.block_cube());
                    let (bx, by, bz) = (dx * SECTION_SIZE, dy * SECTION_SIZE, dz * SECTION_SIZE);
                    super::shape::collect_light_overrides(
                        section,
                        |lx, ly, lz| cube_idx(dim, bx + lx, by + ly, bz + lz),
                        &mut states,
                    );
                }
            }
        }
        Self {
            span,
            blocks,
            states,
        }
    }

    #[inline]
    pub fn dim(&self) -> usize {
        self.span * SECTION_SIZE
    }

    #[inline]
    pub fn volume(&self) -> usize {
        let dim = self.dim();
        dim * dim * dim
    }

    pub fn states(&self) -> &[SparseCellState] {
        &self.states
    }

    pub fn shape_states(&self) -> ShapeStateSnapshot {
        ShapeStateSnapshot::from_sparse(&self.states, self.volume())
    }

    pub fn assemble_blocks(&self, out: &mut [u16]) {
        debug_assert_eq!(out.len(), self.volume());
        let (span, dim) = (self.span, self.dim());
        out.fill(0);
        for dy in 0..span {
            for dz in 0..span {
                for dx in 0..span {
                    let Some(src) = &self.blocks[span_idx(span, dx, dy, dz)] else {
                        continue;
                    };
                    let (bx, by, bz) = (dx * SECTION_SIZE, dy * SECTION_SIZE, dz * SECTION_SIZE);
                    for ly in 0..SECTION_SIZE {
                        for lz in 0..SECTION_SIZE {
                            let d = cube_idx(dim, bx, by + ly, bz + lz);
                            let s = section_idx(0, ly, lz);
                            src.expand_row_into(s, &mut out[d..d + SECTION_SIZE]);
                        }
                    }
                }
            }
        }
    }
}

pub fn collect_emitters(
    low: SectionPos,
    span: usize,
    sections: &FxHashMap<SectionPos, Arc<Section>>,
) -> Vec<(IVec3, LightRgb)> {
    let mut emitters = Vec::new();
    for dy in 0..span {
        for dz in 0..span {
            for dx in 0..span {
                let npos = window_pos(low, dx, dy, dz);
                if let Some(section) = sections.get(&npos) {
                    collect_section_emitters(npos, section, &mut emitters);
                }
            }
        }
    }
    emitters
}

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
