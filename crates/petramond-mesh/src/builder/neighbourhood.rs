//! The section mesher's ONE view of the world around a section: its
//! one-cell-padded snapshot ([`SectionMeshPad`]) addressed by world
//! coordinate, plus every derived neighbourhood query the emitters share —
//! slab stacks, fluid fills, the cube cull, the floor seal, sub-cell AO
//! matter and neighbour occupancy boxes.
//!
//! Each query is a method, so no query captures another and each one is
//! answered in exactly one place for every emitter (cube faces, box sets,
//! fluids, the exposure masks). Reads outside the pad fall back the way the
//! live world's accessors do: air, no state, open sky, not loaded.

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
    /// World coordinates of the section's minimum cell.
    origin: IVec3,
    /// The dispatch tables the build runs against.
    registry: &'a MeshRegistry,
    /// Scratch for [`Self::occupancy_boxes`] and [`Self::seals_floor`].
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

    /// The pad index of world cell `p`, if the pad holds it.
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

    /// Baked skylight: open sky above the world, dark below it.
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

    /// A dyed or snow-covered cell neither gives nor takes transition material.
    #[inline]
    pub(super) fn transition_blocked(&self, p: IVec3) -> bool {
        self.pad_index(p)
            .is_none_or(|i| self.pad.transition_blocked[i])
    }

    /// "Cell holds a full slab stack" — callers gate on `is_slab` first (dense
    /// flag) so this only pays a state lookup on actual slab cells. Full stacks
    /// cull and occlude AO/light like opaque cubes; no normalize needed (a
    /// normalized default is a single layer, never full).
    #[inline]
    fn full_slab(&self, p: IVec3) -> bool {
        SlabState::from_cell(self.cell_state(p)).is_full()
    }

    /// A full opaque occupier: an opaque cube or a full slab stack — the
    /// classic whole-face cull.
    #[inline]
    pub(super) fn solid(&self, p: IVec3) -> bool {
        let f = self.flags(p);
        f.is_opaque() || (f.is_slab() && self.full_slab(p))
    }

    // --- Fluid probes -------------------------------------------------------
    //
    // Each takes the fluid it reads: corner heights and fills only ever average
    // cells of the SAME fluid, so lava beside water renders both.

    #[inline]
    fn local(&self, p: IVec3) -> IVec3 {
        p - self.origin
    }

    /// Whether `p` holds `fluid` filling its whole cell.
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

    // --- Culling -------------------------------------------------------------

    /// "Does the cell at `p` seal the boundary beneath it?" — the cube path's
    /// only sub-cell neighbour cull, asked of the NEIGHBOUR's own resolved
    /// boxes (`boxset::cell_seals_face`), so it names no family and a mod shape
    /// with a floor-flush base gets it for free. A sealed top face is
    /// invisible, and a nearly-coplanar one is worse than invisible: a snow
    /// layer's top sits 1/16 above its carrier's and the two z-fight from far
    /// above. Deliberately PosY-only — a sealed face in the other five
    /// directions is plain overdraw, never a visible artifact.
    pub(super) fn seals_floor(&self, p: IVec3) -> bool {
        let (boxes, scratch) = &mut *self.scratch.seal.borrow_mut();
        cell_seals_face(self, p, Face::NegY, boxes, scratch)
    }

    /// Whether the cell at `p` hides a cube `face` pointing into it. A full
    /// cell of an opaque medium hides the faces behind it like stone: the rock
    /// walls of a lava sea are never meshed. See-through fluids cover nothing.
    pub(super) fn covers_face(&self, p: IVec3, face: Face) -> bool {
        let b = self.block(p);
        let f = self.pad.table.flags(b.id());
        f.is_opaque()
            || (f.is_slab() && self.full_slab(p))
            || (self.registry.pad_class(b.id()) & PAD_OPAQUE_FLUID != 0 && self.fluid_fills(p, b))
            || (matches!(face, Face::PosY) && self.seals_floor(p))
    }

    /// The cell-local occupancy boxes of the block at `p` — what the box-set
    /// emitter subtracts from a flush face so sub-cell geometry culls against
    /// sub-cell geometry (a fence cap on a slab, a chain continuing into the
    /// chain above). Whole opaque cells are handled by the cheaper solid cull
    /// and contribute nothing here; families with no box form (plants, torch,
    /// models, custom bakes across the section boundary) stay empty, which
    /// just means "no sub-cell cull", never a wrong cull.
    pub(super) fn occupancy_boxes(
        &self,
        p: IVec3,
        cell_block: Block,
        out: &mut Vec<([f32; 3], [f32; 3])>,
    ) {
        let nb_block = self.block(p);
        // Dense flag first: this runs per face of every box-shaped cell, and
        // the shape-kind row behind `mesh_emitter` is a big-table load that
        // almost every neighbour is rejected without needing.
        if !nb_block.has_box_shape() {
            return;
        }
        // See-through texels cannot seal ANOTHER block's face: a cutout ladder
        // panel flush on a stair would cull the stair's side and show a hole
        // through the rungs. Same-block contact still culls — the
        // glass/translucent convention the cube path uses — so stacked
        // panes/chains keep their exact box-vs-box culls.
        if (nb_block.is_transparent() || nb_block.is_translucent()) && nb_block != cell_block {
            return;
        }
        // The neighbour's own resolved boxes — the SAME producer the mesh uses,
        // so what culls a face is exactly what would have been drawn there.
        // Presentation is irrelevant to an occupancy query, so the tint is a
        // constant; the scratch keeps the per-face call allocation free.
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
        // A posed box lies on no axis plane, so it can seal nothing flush.
        out.extend(
            boxes
                .iter()
                .filter(|b| b.occludes && b.pose.is_none())
                .map(|b| (b.aabb.min, b.aabb.max)),
        );
    }

    /// The shared sub-cell AO occupancy query: does the cell hold solid matter
    /// overlapping the cell-local pocket AABB? Whole cell for opaque cubes /
    /// full stacks; otherwise the FAMILY's answer (`ShapeSim::shades_pocket`):
    /// half-cell refined state for partial slabs and stairs, the post for
    /// fences/panes, the panel for ladders, the render-bake boxes for
    /// in-section custom cells. Consumed by the face-lighting cast probes AND
    /// the box emitter's out-of-cell probes, so casting and receiving are one
    /// rule.
    pub(super) fn matter(&self, p: IVec3, lo: [f32; 3], hi: [f32; 3]) -> bool {
        let b = self.block(p);
        if self.pad.table.flags(b.id()).occludes_ao() {
            return true;
        }
        let k = b.shape_kind_def();
        k.sim.shades_pocket(&k.params, self, p, b, lo, hi)
    }
}

/// The primitive shape seam over the pad: a shape family resolves here through
/// exactly the seam it uses on the sim thread, so one implementation serves
/// both. A mesh job runs on a worker thread over the padded snapshot and has
/// no `&World`.
impl petramond_world::block::ShapeNeighborhood for Neighbourhood<'_> {
    fn block(&self, pos: IVec3) -> Block {
        Neighbourhood::block(self, pos)
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        // ONE read of the unified store's pad capture — the seam ships the
        // bytes verbatim; only the family owning the cell's block decodes.
        self.cell_state(pos)
    }

    fn baked(&self, pos: IVec3) -> Option<&[petramond_world::block::ShapeRenderBox]> {
        // Only this section's bakes are in the snapshot; a custom neighbour
        // across the boundary reads as unbaked, which means "no sub-cell
        // cull" — never a wrong one.
        let l = pos - self.origin;
        let r = 0..SECTION_SIZE as i32;
        if !(r.contains(&l.x) && r.contains(&l.y) && r.contains(&l.z)) {
            return None;
        }
        self.section
            .shape_render_boxes(section_idx(l.x as usize, l.y as usize, l.z as usize) as u16)
    }
}
