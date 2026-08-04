//! bbmodel blocks at the world level: position-aware collision/selection, multi-cell
//! placement gating, and the footprint group for breaking.
//!
//! A bbmodel block's collision and selection are PER CELL — a multi-block (the workbench
//! is 2×2×1) splits its shape across its footprint, and a cell's shape depends on its
//! authored offset plus placed facing, which only the world knows (the chunk model maps).
//! So the per-cell queries live here, over the chunk-owned placement metadata, while
//! [`Block`]'s own (position-less) accessors answer the authored-origin cell. See
//! [`crate::block_model`].
//! (Data-half queries; the mutation/orchestration half stays in the engine crate.)

use crate::world::data::WorldData;
use crate::block::{Aabb, Block};
use crate::block_model::{self, BlockModelKind};
use crate::facing::Facing;
use crate::mathh::{IVec3, Mat4, Vec3};
    
    

impl WorldData {
    /// The authored footprint offset of the model-block cell at world `pos` —
    /// `[0,0,0]` for the authored-origin cell, a single-cell model, or a non-model cell.
    #[inline]
    pub fn model_offset_at(&self, wx: i32, wy: i32, wz: i32) -> [u8; 3] {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.model_offset(lx, ly, lz),
            None => [0, 0, 0],
        }
    }

    /// The placed facing of the model-block cell at world `pos`. Old/non-oriented
    /// placements default to the canonical unrotated bbmodel facing.
    #[inline]
    pub fn model_facing_at(&self, wx: i32, wy: i32, wz: i32) -> Facing {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.model_facing(lx, ly, lz),
            None => block_model::DEFAULT_MODEL_FACING,
        }
    }

    /// Position-aware player-collision boxes: a bbmodel block resolves its PER-CELL
    /// boxes (footprint offset → cell-local shape); every other block uses its block
    /// default. Drives the player movement sweep (`player::movement`) and any other
    /// collision that must hug a multi-block correctly.
    #[inline]
    pub fn collision_boxes_at(&self, wx: i32, wy: i32, wz: i32) -> &'static [Aabb] {
        // The per-shape resolve lives on the shape's `ShapeSim` facet (stateful
        // families read their per-cell state / neighbours off `self`; the rest
        // fall to the row's position-less boxes). Adding a shape adds a facet
        // impl, not an arm here — see `block::shape_kind`.
        let block = self.physics_block(wx, wy, wz);
        // Plain terrain (cube, plant, crop, torch) collides as its block id
        // alone says, so it answers from the dense per-id table without a
        // shape lookup or a virtual resolve.
        if let Some(boxes) = block.static_collision_boxes() {
            return boxes;
        }
        let k = block.shape_kind_def();
        k.sim
            .collision_boxes(&k.params, self, IVec3::new(wx, wy, wz), block)
    }

    /// Position-aware selection/TARGET box: a bbmodel block resolves its PER-CELL box
    /// (the geometry overlapping that cell, so the raycast targets where the model
    /// actually is); every other block uses its default ([`Block::visual_aabb`]). Drives
    /// the raycast target test and the break overlay. The DRAWN outline of a model block
    /// is the whole-model box — see [`model_outline_box`](Self::model_outline_box).
    #[inline]
    pub fn selection_box_at(&self, wx: i32, wy: i32, wz: i32) -> Option<([f32; 3], [f32; 3])> {
        // Mirror of `collision_boxes_at` on the `ShapeRender` facet: the
        // targeting box must agree with the real collision box, so both derive
        // per-shape from the same per-cell state. See `block::shape_kind`.
        let block = Block::from_id(self.chunk_block(wx, wy, wz));
        let k = block.shape_kind_def();
        k.render
            .selection_box(&k.params, self, IVec3::new(wx, wy, wz), block)
    }

    /// Is world-space point `p` inside a real collision box of its cell? The model-aware
    /// point test particles settle against — built on [`collision_boxes_at`](Self::collision_boxes_at)
    /// so a particle stops on a bbmodel block's actual leg/top, and drifts through the
    /// empty space around it, exactly like the player/mob/item bodies. (Bodies use
    /// [`crate::collision::resolve_body`] over the same box source; this is the point case.)
    #[inline]
    pub fn point_blocked(&self, p: crate::mathh::Vec3) -> bool {
        crate::collision::point_in_solid([p.x, p.y, p.z], |x, y, z| {
            self.collision_boxes_at(x, y, z)
        })
    }

    /// The WORLD-space black-outline box for the model block at `pos`: the model's tight
    /// bounding box (baked from geometry) positioned at its rotated-footprint base, so the
    /// wireframe traces the whole multi-block as ONE box hugging its real extent rather
    /// than a per-cell cube. `None` for a non-model cell.
    pub fn model_outline_box(&self, pos: IVec3) -> Option<([f32; 3], [f32; 3])> {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        let kind = block.model_kind()?;
        let off = self.model_offset_at(pos.x, pos.y, pos.z);
        let facing = self.model_facing_at(pos.x, pos.y, pos.z);
        let base = block_model::base_from_cell(pos, kind, off, facing);
        let (mn, mx) = block_model::outline_bounds(kind);
        let m = block_model::placement_transform(base, kind, facing);
        Some(transform_box(m, mn, mx))
    }

    /// The cells a `kind` block placed with its rotated-footprint base at `base` occupies —
    /// only the cells the model actually fills (its split produced geometry/collision/
    /// selection for), so an empty corner of a non-rectangular footprint is never a
    /// phantom solid. Placement, gating, and breaking all operate over exactly these.
    pub fn model_footprint_cells(base: IVec3, kind: BlockModelKind) -> Vec<IVec3> {
        Self::model_footprint_cells_facing(base, kind, block_model::DEFAULT_MODEL_FACING)
    }

    /// Oriented form of [`model_footprint_cells`](Self::model_footprint_cells).
    pub fn model_footprint_cells_facing(
        base: IVec3,
        kind: BlockModelKind,
        facing: Facing,
    ) -> Vec<IVec3> {
        block_model::oriented_footprint_cells(base, kind, facing)
            .into_iter()
            .map(|(cell, _)| cell)
            .collect()
    }

    /// Whether every footprint cell for a `kind` block at `origin` is loaded and
    /// replaceable (air/water) — the WORLD half of the placement gate. The caller adds
    /// the entity-overlap gate (player/mobs) against the same cells.
    pub fn model_footprint_clear(&self, origin: IVec3, kind: BlockModelKind) -> bool {
        self.model_footprint_clear_facing(origin, kind, block_model::DEFAULT_MODEL_FACING)
    }

    /// Oriented form of [`model_footprint_clear`](Self::model_footprint_clear).
    pub fn model_footprint_clear_facing(
        &self,
        base: IVec3,
        kind: BlockModelKind,
        facing: Facing,
    ) -> bool {
        Self::model_footprint_cells_facing(base, kind, facing)
            .into_iter()
            .all(|c| self.placement_cell_open(c))
    }

}

fn transform_box(m: Mat4, min: [f32; 3], max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mn = Vec3::from(min);
    let mx = Vec3::from(max);
    let mut out_min = Vec3::splat(f32::INFINITY);
    let mut out_max = Vec3::splat(f32::NEG_INFINITY);
    for x in [mn.x, mx.x] {
        for y in [mn.y, mx.y] {
            for z in [mn.z, mx.z] {
                let p = m.transform_point3(Vec3::new(x, y, z));
                out_min = out_min.min(p);
                out_max = out_max.max(p);
            }
        }
    }
    (out_min.to_array(), out_max.to_array())
}
