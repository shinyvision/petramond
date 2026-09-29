use glam::IVec3;
use petramond_world::block::{Aabb, Block, BlockFlags, CellView, ShapeState};
use petramond_world::block_state::SlabState;
use petramond_world::chunk::{section_idx, SECTION_SIZE, SKY_FULL, WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::light::LightRgb;
use petramond_world::section::Section;
use petramond_world::tile::Tile;

use super::super::boxset::cell_seals_face;
use super::super::face::Face;
use super::cell_class::{
    MeshRegistry, FAST_CUBE, PAD_OPAQUE, PAD_OPAQUE_FLUID, PAD_SEALS, PAD_SLAB, SKIP,
};
use super::pad::{mesh_pad_idx, SectionMeshPad, SECTION_PAD};
use super::scratch::NeighbourScratch;

pub(super) struct Neighbourhood<'a> {
    pad: &'a SectionMeshPad<'a>,
    section: &'a Section,
    origin: IVec3,
    registry: &'a MeshRegistry,
    scratch: &'a NeighbourScratch,
    ring: &'a [u32],
}

/// Skylight bits of a packed ring word.
pub(super) const RING_SKY_MASK: u32 = 0x1F;
pub(super) const RING_BLOCK_SHIFT: u32 = 5;
pub(super) const RING_BLOCK_MASK: u32 = 0x7FFF;
pub(super) const RING_CLASS_SHIFT: u32 = 20;

/// Per-row cell bitsets the exposure-mask builder needs, gathered in the ring pack's one pass
/// over the pad instead of two more passes of their own.
pub(super) struct RowBits {
    /// Per pad `(py, pz)` row: the cells that seal a cube face against them — opaque cubes,
    /// full slab stacks, filling opaque fluid.
    pub(super) opaque: [u32; SECTION_PAD * SECTION_PAD],
    /// Per pad row: the non-sealing cells whose own geometry may seal the boundary beneath
    /// them (`PAD_SEALS`), resolved per cell by the mask builder.
    pub(super) seals: [u32; SECTION_PAD * SECTION_PAD],
    /// Per interior `(ly, lz)` row: the cells the scan visits at all (not `SKIP`).
    pub(super) work: [u32; SECTION_SIZE * SECTION_SIZE],
    /// Per interior row: the cube fast-path candidates (cube-drawn rows and uniform full slab
    /// stacks), whose faces the exposure masks cull.
    pub(super) candidate: [u32; SECTION_SIZE * SECTION_SIZE],
}

impl Default for RowBits {
    fn default() -> Self {
        Self {
            opaque: [0; SECTION_PAD * SECTION_PAD],
            seals: [0; SECTION_PAD * SECTION_PAD],
            work: [0; SECTION_SIZE * SECTION_SIZE],
            candidate: [0; SECTION_SIZE * SECTION_SIZE],
        }
    }
}

/// Pack every pad cell into one word — skylight (5 bits), block light (15) and the ring class
/// (`MeshRegistry::ring_class`) — so the per-face gathers read one load per ring cell instead of
/// the block id, its flags row and two light cells. The same pass gathers the [`RowBits`].
pub(super) fn pack_ring(
    ring: &mut Vec<u32>,
    rows: &mut RowBits,
    pad: &SectionMeshPad<'_>,
    registry: &MeshRegistry,
) {
    let n = pad.blocks.len();
    debug_assert_eq!(n, SECTION_PAD * SECTION_PAD * SECTION_PAD);
    ring.clear();
    ring.reserve(n);
    let mut i = 0;
    for py in 0..SECTION_PAD {
        for pz in 0..SECTION_PAD {
            let (mut opaque_row, mut seals_row, mut work_row, mut cand_row) =
                (0u32, 0u32, 0u32, 0u32);
            let interior_y = (1..SECTION_PAD - 1).contains(&py);
            let interior_z = (1..SECTION_PAD - 1).contains(&pz);
            for px in 0..SECTION_PAD {
                debug_assert_eq!(i, mesh_pad_idx(px, py, pz));
                let id = pad.blocks[i];
                let cls = registry.ring_class(id);
                ring.push(
                    u32::from(pad.skylight[i])
                        | (u32::from(pad.blocklight[i].bits()) << RING_BLOCK_SHIFT)
                        | ((cls as u32) << RING_CLASS_SHIFT),
                );
                let bit = 1u32 << px;
                if cls & PAD_OPAQUE != 0
                    || (cls & PAD_SLAB != 0 && SlabState::from_cell(pad.cell_states[i]).is_full())
                    || (cls & PAD_OPAQUE_FLUID != 0
                        && pad.fluid_fills_local(
                            px as i32 - 1,
                            py as i32 - 1,
                            pz as i32 - 1,
                            pad.table.block(id),
                        ))
                {
                    opaque_row |= bit;
                } else if cls & PAD_SEALS != 0 {
                    seals_row |= bit;
                }
                if interior_y && interior_z && (1..SECTION_PAD - 1).contains(&px) {
                    let cell = registry.cell_class(id);
                    let lbit = 1u32 << (px - 1);
                    if cell & SKIP == 0 {
                        work_row |= lbit;
                    }
                    if cell & FAST_CUBE != 0
                        || (registry.pad_class(id) & PAD_SLAB != 0
                            && petramond_world::slab::is_uniform_full_stack(SlabState::from_cell(
                                pad.cell_states[i],
                            )))
                    {
                        cand_row |= lbit;
                    }
                }
                i += 1;
            }
            let row = py * SECTION_PAD + pz;
            rows.opaque[row] = opaque_row;
            rows.seals[row] = seals_row;
            if interior_y && interior_z {
                let lrow = (py - 1) * SECTION_SIZE + (pz - 1);
                rows.work[lrow] = work_row;
                rows.candidate[lrow] = cand_row;
            }
        }
    }
}

/// The shade boxes of every box-shape cell a build has probed so far, by pad index: the AO
/// pockets ask the same few neighbour cells hundreds of times per section, and each ask used to
/// re-resolve the neighbour's box set through its family. Generation-stamped, so a new build
/// costs no clear.
#[derive(Default)]
pub(super) struct ShadeCache {
    gen: u32,
    index: Vec<ShadeEntry>,
    boxes: Vec<Aabb>,
    scratch: Vec<Aabb>,
}

#[derive(Copy, Clone, Default)]
struct ShadeEntry {
    gen: u32,
    start: u32,
    len: u16,
    listable: bool,
}

impl ShadeCache {
    fn begin(&mut self, cells: usize) {
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.index.clear();
            self.gen = 1;
        }
        self.index.resize(cells, ShadeEntry::default());
        self.boxes.clear();
    }
}

impl<'a> Neighbourhood<'a> {
    pub(super) fn new(
        pad: &'a SectionMeshPad<'a>,
        section: &'a Section,
        origin: IVec3,
        registry: &'a MeshRegistry,
        scratch: &'a NeighbourScratch,
        ring: &'a [u32],
    ) -> Self {
        debug_assert_eq!(ring.len(), pad.blocks.len());
        scratch.shade.borrow_mut().begin(pad.blocks.len());
        scratch.vertex_light.borrow_mut().begin();
        Self {
            pad,
            section,
            origin,
            registry,
            scratch,
            ring,
        }
    }

    #[inline]
    pub(super) fn ring(&self) -> &'a [u32] {
        self.ring
    }

    #[inline]
    pub(super) fn vertex_light(&self) -> &'a std::cell::RefCell<super::lighting::VertexLightCache> {
        &self.scratch.vertex_light
    }

    #[inline]
    pub(super) fn registry(&self) -> &'a MeshRegistry {
        self.registry
    }

    #[inline]
    pub(super) fn pad(&self) -> &SectionMeshPad<'a> {
        self.pad
    }

    #[inline]
    pub(super) fn origin(&self) -> IVec3 {
        self.origin
    }

    #[inline]
    pub(super) fn pad_index(&self, p: IVec3) -> Option<usize> {
        let q = p - self.origin + IVec3::ONE;
        let n = SECTION_PAD as i32;
        ((0..n).contains(&q.x) && (0..n).contains(&q.y) && (0..n).contains(&q.z))
            .then(|| mesh_pad_idx(q.x as usize, q.y as usize, q.z as usize))
    }

    #[inline]
    pub(super) fn block_id(&self, p: IVec3) -> u16 {
        self.pad_index(p).map_or(0, |i| self.pad.blocks[i])
    }

    #[inline]
    pub(super) fn block(&self, p: IVec3) -> Block {
        self.pad.table.block(self.block_id(p))
    }

    #[inline]
    fn flags(&self, p: IVec3) -> BlockFlags {
        self.pad.table.flags(self.block_id(p))
    }

    #[inline]
    pub(super) fn cell_state(&self, p: IVec3) -> ShapeState {
        self.pad_index(p)
            .map_or(ShapeState::NONE, |i| self.pad.cell_states[i])
    }

    #[inline]
    pub(super) fn skylight(&self, p: IVec3) -> u8 {
        if p.y >= WORLD_MAX_Y {
            return SKY_FULL;
        }
        if p.y < WORLD_MIN_Y {
            return 0;
        }
        self.pad_index(p).map_or(SKY_FULL, |i| self.pad.skylight[i])
    }

    #[inline]
    pub(super) fn blocklight(&self, p: IVec3) -> LightRgb {
        self.pad_index(p)
            .map_or(LightRgb::ZERO, |i| self.pad.blocklight[i])
    }

    #[inline]
    pub(super) fn loaded(&self, p: IVec3) -> bool {
        self.pad_index(p).is_some_and(|i| self.pad.loaded[i])
    }

    #[inline]
    pub(super) fn transition_blocked(&self, p: IVec3) -> bool {
        self.pad_index(p)
            .is_none_or(|i| self.pad.transition_blocked[i])
    }

    #[inline]
    fn full_slab(&self, p: IVec3) -> bool {
        SlabState::from_cell(self.cell_state(p)).is_full()
    }

    #[inline]
    pub(super) fn solid(&self, p: IVec3) -> bool {
        let f = self.flags(p);
        f.is_opaque() || (f.is_slab() && self.full_slab(p))
    }

    #[inline]
    fn local(&self, p: IVec3) -> IVec3 {
        p - self.origin
    }

    #[inline]
    pub(super) fn fluid_fills(&self, p: IVec3, fluid: Block) -> bool {
        let l = self.local(p);
        self.pad.fluid_fills_local(l.x, l.y, l.z, fluid)
    }

    #[inline]
    pub(super) fn fluid_still(&self, p: IVec3, fluid: Block) -> bool {
        let l = self.local(p);
        self.pad.fluid_still_local(l.x, l.y, l.z, fluid)
    }

    #[inline]
    pub(super) fn fluid_falling(&self, p: IVec3, fluid: Block) -> bool {
        let l = self.local(p);
        self.pad.fluid_falling_local(l.x, l.y, l.z, fluid)
    }

    #[inline]
    pub(super) fn fluid_height(&self, p: IVec3, fluid: Block) -> Option<f32> {
        let l = self.local(p);
        self.pad.fluid_height_local(l.x, l.y, l.z, fluid)
    }

    pub(super) fn seals_floor(&self, p: IVec3) -> bool {
        let (boxes, scratch) = &mut *self.scratch.seal.borrow_mut();
        cell_seals_face(self, p, Face::NegY, boxes, scratch)
    }

    pub(super) fn covers_face(&self, p: IVec3, face: Face) -> bool {
        let b = self.block(p);
        let f = self.pad.table.flags(b.id());
        f.is_opaque()
            || (f.is_slab() && self.full_slab(p))
            || (self.registry.pad_class(b.id()) & PAD_OPAQUE_FLUID != 0 && self.fluid_fills(p, b))
            || (matches!(face, Face::PosY) && self.seals_floor(p))
    }

    pub(super) fn occupancy_boxes(
        &self,
        p: IVec3,
        cell_block: Block,
        out: &mut Vec<([f32; 3], [f32; 3])>,
    ) {
        let nb_block = self.block(p);
        if !nb_block.has_box_shape() {
            return;
        }
        if (nb_block.is_transparent() || nb_block.is_translucent()) && nb_block != cell_block {
            return;
        }
        let k = nb_block.shape_kind_def();
        let mut boxes = self.scratch.occupancy.borrow_mut();
        boxes.clear();
        let tint_for = |_: Tile| [1.0f32; 3];
        k.render.boxes(
            &petramond_world::block::ShapeCtx {
                nb: self,
                pos: p,
                block: nb_block,
                params: &k.params,
                tint_for: &tint_for,
                part_tint: petramond_world::block::NO_PART_TINT,
            },
            &mut boxes,
        );
        out.extend(
            boxes
                .iter()
                .filter(|b| b.occludes && b.pose.is_none())
                .map(|b| (b.aabb.min, b.aabb.max)),
        );
    }

    pub(super) fn matter(&self, p: IVec3, lo: [f32; 3], hi: [f32; 3]) -> bool {
        let b = self.block(p);
        let flags = self.pad.table.flags(b.id());
        if flags.occludes_ao() {
            return true;
        }
        // Only a box-shape family answers `shades_pocket` with anything but the `false`
        // default; the dense flag spares air (the usual ring cell) a registry row load and a
        // virtual call per probe.
        if !flags.has_box_shape() {
            return false;
        }
        let k = b.shape_kind_def();
        let Some(pi) = self.pad_index(p) else {
            return k.sim.shades_pocket(&k.params, self, p, b, lo, hi);
        };
        let mut cache = self.scratch.shade.borrow_mut();
        let ShadeCache {
            gen,
            index,
            boxes,
            scratch,
        } = &mut *cache;
        let mut entry = index[pi];
        if entry.gen != *gen {
            scratch.clear();
            let listable = k.sim.shade_boxes(&k.params, self, p, b, scratch);
            entry = ShadeEntry {
                gen: *gen,
                start: boxes.len() as u32,
                len: if listable { scratch.len() as u16 } else { 0 },
                listable,
            };
            if listable {
                boxes.extend_from_slice(scratch);
            }
            index[pi] = entry;
        }
        if !entry.listable {
            drop(cache);
            return k.sim.shades_pocket(&k.params, self, p, b, lo, hi);
        }
        let (start, len) = (entry.start as usize, entry.len as usize);
        boxes[start..start + len]
            .iter()
            .any(|bx| (0..3).all(|a| lo[a] < bx.max[a] && hi[a] > bx.min[a]))
    }
}

impl petramond_world::block::ShapeNeighborhood for Neighbourhood<'_> {
    fn block(&self, pos: IVec3) -> Block {
        Neighbourhood::block(self, pos)
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        self.cell_state(pos)
    }

    fn baked(&self, pos: IVec3) -> Option<&[petramond_world::block::ShapeRenderBox]> {
        let l = pos - self.origin;
        let r = 0..SECTION_SIZE as i32;
        if !(r.contains(&l.x) && r.contains(&l.y) && r.contains(&l.z)) {
            return None;
        }
        self.section
            .shape_render_boxes(section_idx(l.x as usize, l.y as usize, l.z as usize) as u16)
    }
}
