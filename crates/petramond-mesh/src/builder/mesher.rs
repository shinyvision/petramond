//! The section mesher: one pass over a section's cells that hands each cell
//! to the emitter its render family names, then merges the deferred flat cube
//! faces and splits the far LOD off the opaque stream.
//!
//! The stages live in their own modules — face culling (`exposed_masks` and
//! `neighbourhood`), AO/light (`lighting`), cube faces and their greedy
//! deferral (`cube`), fluids (`fluid_faces`) and the plant / pole / box-set /
//! model families (`families`) — and share this struct's state: the
//! neighbourhood view, the section's tints, the scratch buffers and the
//! output streams.

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

/// One cell the scan hands to an emitter.
#[derive(Copy, Clone)]
pub(super) struct Cell {
    /// The block being drawn: the resident block, or the fluid it contains.
    pub(super) block: Block,
    /// Whether `block` is the cell's resident (not a contained fluid).
    pub(super) resident: bool,
    pub(super) lx: usize,
    pub(super) ly: usize,
    pub(super) lz: usize,
    pub(super) world: IVec3,
    /// Section cell index.
    pub(super) idx: usize,
    /// Column index within the section's 16×16 footprint.
    pub(super) column: usize,
}

pub(super) struct SectionMesher<'a> {
    pub(super) section: &'a Section,
    pub(super) nb: Neighbourhood<'a>,
    pub(super) rules: &'a Rules,
    pub(super) tints: CellTinting,
    /// Every vertex is emitted in MESH space: column-local X/Z, world Y. An
    /// absolute coordinate never becomes a float, so a section meshes
    /// identically however far out it lies; the draw adds the column's integer
    /// origin back relative to the camera.
    pub(super) anchor: IVec3,
    pub(super) out: &'a mut Streams,
    pub(super) boxes: &'a mut BoxBuffers,
    /// Flat opaque cube faces deferred during the scan, merged into tiled
    /// quads after it.
    pub(super) greedy: &'a mut GreedyScratch,
    pub(super) greedy_gen: u32,
}

/// Mesh one section from its pad. `exposure_masks` selects the cube cull:
/// the production build takes the exposure-mask fast path (whole buried rows
/// skipped, faces culled from bitsets); `false` culls every cube face through
/// [`Neighbourhood::covers_face`] instead, which must give byte-identical
/// output — the parity the mesher tests hold the fast path to. `None` when
/// `cancelled` fires between section rows.
///
/// Every buffer comes from this thread's leased [`MeshScratch`], which goes
/// back to the thread on every exit path.
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
    } = &mut *lease;
    let greedy_gen = greedy.begin();
    let mut mesher = SectionMesher {
        section,
        nb: Neighbourhood::new(
            pad,
            section,
            IVec3::new(ox, oy, oz),
            ctx.registry,
            neighbour,
        ),
        rules: ctx.rules,
        tints: CellTinting::new(section, biome),
        anchor: IVec3::new(ox, 0, oz),
        out,
        boxes,
        greedy,
        greedy_gen,
    };
    let masks = exposure_masks.then(|| build_exposed_masks(&mesher.nb));
    if !mesher.scan(masks.as_ref(), cancelled) {
        return None;
    }
    emit_greedy_quads(mesher.greedy, &mut mesher.out.opaque, IVec3::new(0, oy, 0));
    Some(finish(mesher.out))
}

impl SectionMesher<'_> {
    /// Visit every cell with work, row by row. Which cells in each `(ly, lz)`
    /// row have any work at all comes from the exposure masks when present:
    /// they exclude buried cubes outright, so a solid underground row costs
    /// one word test instead of sixteen classified cells. `false` when
    /// cancelled.
    fn scan(&mut self, masks: Option<&ExposedMasks>, cancelled: &dyn Fn() -> bool) -> bool {
        let visit = masks.map_or(&VISIT_ALL, ExposedMasks::visit_rows);
        let registry = self.nb.registry();
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
                    let resident = Block::from_id(self.section.block_raw(lx, ly, lz));
                    for block in std::iter::once(resident).chain(resident.contained_fluid()) {
                        // Dense per-id tables answer every dispatch question
                        // (see `cell_class`): the class byte skips air and rows
                        // drawn outside the chunk mesh, the emitter table names
                        // the family.
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

    /// Hand one cell to the emitter its row declares. A fluid (resident or
    /// contained) always meshes as a fluid; a box family whose resolved form
    /// is the full cube, or that resolved no boxes, falls to the cube path.
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

/// Close a finished build into exact-size copies of the scratch streams: the
/// far LOD is everything emitted so far and the leaf internals follow it. A
/// section with none of them has no far LOD to offer (0 = "no far mesh").
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
