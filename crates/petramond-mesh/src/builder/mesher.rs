use glam::IVec3;
use petramond_world::block::{Block, MeshEmitter};
use petramond_world::chunk::{section_idx, SectionPos, SECTION_SIZE};
use petramond_world::section::Section;
use petramond_world::texture_transition::Rules;

use super::super::greedy::{emit_greedy_quads, GreedyScratch};
use super::super::tint;
use super::super::vertex::ChunkMesh;
use super::cell_class::{FAST_CUBE, FLUID, SKIP};
use super::cell_tint::CellTinting;
use super::exposed_masks::{build_exposed_masks, ExposedMasks, VISIT_ALL};
use super::families::BoxesOutcome;
use super::neighbourhood::Neighbourhood;
use super::pad::SectionMeshPad;
use super::scratch::{BoxBuffers, MeshScratch, ScratchLease, Streams};
use super::transition;
use super::MeshContext;

#[derive(Copy, Clone)]
pub(super) struct Cell {
    pub(super) block: Block,
    pub(super) resident: bool,
    pub(super) lx: usize,
    pub(super) ly: usize,
    pub(super) lz: usize,
    pub(super) world: IVec3,
    pub(super) idx: usize,
    pub(super) column: usize,
}

pub(super) struct SectionMesher<'a> {
    pub(super) section: &'a Section,
    pub(super) nb: Neighbourhood<'a>,
    pub(super) rules: &'a Rules,
    pub(super) tints: CellTinting,
    pub(super) anchor: IVec3,
    pub(super) out: &'a mut Streams,
    pub(super) boxes: &'a mut BoxBuffers,
    pub(super) greedy: &'a mut GreedyScratch,
    pub(super) greedy_gen: u32,
}

pub(super) fn mesh_section(
    section: &Section,
    pos: SectionPos,
    pad: &SectionMeshPad<'_>,
    ctx: MeshContext<'_>,
    cancelled: &dyn Fn() -> bool,
    exposure_masks: bool,
) -> Option<ChunkMesh> {
    let (ox, oy, oz) = pos.origin_world();
    let biome = transition::needs_tint(section, ctx.rules)
        .then(|| tint::biome_window(ox, oz, |wx, wz| pad.biome_world(ox, oz, wx, wz)));
    let mut lease = ScratchLease::take();
    let MeshScratch {
        greedy,
        boxes,
        neighbour,
        out,
        ring,
        rows,
    } = &mut *lease;
    let greedy_gen = greedy.begin();
    super::neighbourhood::pack_ring(ring, rows, pad, ctx.registry);
    let ring: &[u32] = ring;
    let rows: &super::neighbourhood::RowBits = rows;
    let mut mesher = SectionMesher {
        section,
        nb: Neighbourhood::new(
            pad,
            section,
            IVec3::new(ox, oy, oz),
            ctx.registry,
            neighbour,
            ring,
        ),
        rules: ctx.rules,
        tints: CellTinting::new(section, biome),
        anchor: IVec3::new(ox, 0, oz),
        out,
        boxes,
        greedy,
        greedy_gen,
    };
    let masks = exposure_masks.then(|| build_exposed_masks(&mesher.nb, rows));
    if !mesher.scan(masks.as_ref(), cancelled) {
        return None;
    }
    emit_greedy_quads(mesher.greedy, &mut mesher.out.opaque, IVec3::new(0, oy, 0));
    Some(finish(mesher.out))
}

impl SectionMesher<'_> {
    fn scan(&mut self, masks: Option<&ExposedMasks>, cancelled: &dyn Fn() -> bool) -> bool {
        let visit = masks.map_or(&VISIT_ALL, ExposedMasks::visit_rows);
        let registry = self.nb.registry();
        let table = self.nb.pad().table;
        let origin = self.nb.origin();
        for ly in 0..SECTION_SIZE {
            if cancelled() {
                return false;
            }
            for lz in 0..SECTION_SIZE {
                let mut row = visit[ly * SECTION_SIZE + lz];
                while row != 0 {
                    let lx = row.trailing_zeros() as usize;
                    row &= row - 1;
                    let resident = table.block(self.section.block_raw(lx, ly, lz));
                    for block in
                        std::iter::once(resident).chain(registry.contained_fluid(resident.id()))
                    {
                        let class = registry.cell_class(block.id());
                        if class & SKIP != 0 {
                            continue;
                        }
                        let cell = Cell {
                            block,
                            resident: block == resident,
                            lx,
                            ly,
                            lz,
                            world: origin + IVec3::new(lx as i32, ly as i32, lz as i32),
                            idx: section_idx(lx, ly, lz),
                            column: lz * SECTION_SIZE + lx,
                        };
                        self.emit_cell(&cell, class, registry.emitter(block.id()), masks);
                    }
                }
            }
        }
        true
    }

    fn emit_cell(
        &mut self,
        cell: &Cell,
        class: u8,
        emitter: MeshEmitter,
        masks: Option<&ExposedMasks>,
    ) {
        if class & FLUID != 0 {
            self.emit_fluid(cell);
            return;
        }
        match emitter {
            MeshEmitter::Nothing => {}
            MeshEmitter::Plant(layout) => self.emit_plant(cell, layout),
            MeshEmitter::Pole => self.emit_pole(cell),
            MeshEmitter::Model => self.emit_model(cell),
            MeshEmitter::Boxes => match self.emit_boxes(cell) {
                BoxesOutcome::Drawn => {}
                BoxesOutcome::Cube { whole_stack } => {
                    self.emit_cube(cell, whole_stack, whole_stack.then_some(masks).flatten())
                }
            },
            MeshEmitter::Cube => {
                self.emit_cube(cell, false, masks.filter(|_| class & FAST_CUBE != 0))
            }
        }
    }
}

fn finish(out: &Streams) -> ChunkMesh {
    let far_opaque_len = if out.leaf_interior.is_empty() {
        0
    } else {
        out.opaque.len() as u32
    };
    let mut opaque = Vec::with_capacity(out.opaque.len() + out.leaf_interior.len());
    opaque.extend_from_slice(&out.opaque);
    opaque.extend_from_slice(&out.leaf_interior);
    ChunkMesh {
        opaque,
        far_opaque_len,
        transparent: out.transparent.to_vec(),
        transparent_two_sided: out.transparent_two_sided.to_vec(),
        translucent: out.translucent.to_vec(),
        model: out.model.to_vec(),
        model_idx: out.model_idx.to_vec(),
        model_blend_idx: out.model_blend_idx.to_vec(),
        contact: out.contact.to_vec(),
        mesh_dirty: true,
        ..ChunkMesh::empty()
    }
}
