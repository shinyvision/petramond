use glam::IVec3;
use petramond_world::block::{Block, BlockFlags, CellView, ShapeState};
use petramond_world::block_state::SlabState;
use petramond_world::chunk::{section_idx, SECTION_SIZE, SKY_FULL, WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::light::LightRgb;
use petramond_world::section::Section;
use petramond_world::tile::Tile;

use super::super::boxset::cell_seals_face;
use super::super::face::Face;
use super::cell_class::{MeshRegistry, PAD_OPAQUE_FLUID};
use super::pad::{mesh_pad_idx, SectionMeshPad, SECTION_PAD};
use super::scratch::NeighbourScratch;

pub(super) struct Neighbourhood<'a> {
    pad: &'a SectionMeshPad<'a>,
    section: &'a Section,
    origin: IVec3,
    registry: &'a MeshRegistry,
    scratch: &'a NeighbourScratch,
}

impl<'a> Neighbourhood<'a> {
    pub(super) fn new(
        pad: &'a SectionMeshPad<'a>,
        section: &'a Section,
        origin: IVec3,
        registry: &'a MeshRegistry,
        scratch: &'a NeighbourScratch,
    ) -> Self {
        Self {
            pad,
            section,
            origin,
            registry,
            scratch,
        }
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
        if self.pad.table.flags(b.id()).occludes_ao() {
            return true;
        }
        let k = b.shape_kind_def();
        k.sim.shades_pocket(&k.params, self, p, b, lo, hi)
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
