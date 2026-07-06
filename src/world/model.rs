//! bbmodel blocks at the world level: position-aware collision/selection, multi-cell
//! placement gating, and the footprint group for breaking.
//!
//! A bbmodel block's collision and selection are PER CELL — a multi-block (the workbench
//! is 2×2×1) splits its shape across its footprint, and a cell's shape depends on its
//! authored offset plus placed facing, which only the world knows (the chunk model maps).
//! So the per-cell queries live here, over the chunk-owned placement metadata, while
//! [`Block`]'s own (position-less) accessors answer the authored-origin cell. See
//! [`crate::block_model`].

use crate::block::{Aabb, Block, RenderShape};
use crate::block_model::{self, BlockModelKind};
use crate::furnace::Facing;
use crate::mathh::{IVec3, Mat4, Vec3};

use super::store::World;

impl World {
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
        let block = self.physics_block(wx, wy, wz);
        if let RenderShape::Model(kind) = block.render_shape() {
            return block_model::collision_boxes_oriented(
                kind,
                self.model_offset_at(wx, wy, wz),
                self.model_facing_at(wx, wy, wz),
            );
        }
        if block.render_shape() == RenderShape::Stair {
            return self.stair_boxes_at(wx, wy, wz);
        }
        if block.render_shape() == RenderShape::Slab {
            return self.slab_boxes_at(wx, wy, wz);
        }
        // A door's thin slab sits on its facing edge, swinging to the adjacent edge when
        // open — both read from the chunk door state (see `world::door` / `crate::door`).
        if block.render_shape() == RenderShape::Door {
            if let Some(state) = self.door_state_at(wx, wy, wz) {
                return crate::door::collision_boxes(state);
            }
        }
        block.collision_boxes()
    }

    /// Position-aware selection/TARGET box: a bbmodel block resolves its PER-CELL box
    /// (the geometry overlapping that cell, so the raycast targets where the model
    /// actually is); every other block uses its default ([`Block::visual_aabb`]). Drives
    /// the raycast target test and the break overlay. The DRAWN outline of a model block
    /// is the whole-model box — see [`model_outline_box`](Self::model_outline_box).
    #[inline]
    pub fn selection_box_at(&self, wx: i32, wy: i32, wz: i32) -> Option<([f32; 3], [f32; 3])> {
        let block = Block::from_id(self.chunk_block(wx, wy, wz));
        if let RenderShape::Model(kind) = block.render_shape() {
            return block_model::selection_aabb_oriented(
                kind,
                self.model_offset_at(wx, wy, wz),
                self.model_facing_at(wx, wy, wz),
            );
        }
        if block.render_shape() == RenderShape::Stair {
            return Some(([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]));
        }
        if block.render_shape() == RenderShape::Slab {
            return self.slab_visual_aabb_at(wx, wy, wz);
        }
        // A door targets the thin slab where it actually is (closed/open edge), so the
        // raycast + break overlay hug the panel rather than the whole cell.
        if block.render_shape() == RenderShape::Door {
            if let Some(state) = self.door_state_at(wx, wy, wz) {
                return Some(crate::door::selection_aabb(state));
            }
        }
        block.visual_aabb()
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
        let RenderShape::Model(kind) = block.render_shape() else {
            return None;
        };
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

    /// Place model `block` with its rotated-footprint base at `base`: write the block id to
    /// every footprint cell and record each non-zero authored offset, THEN relight +
    /// remesh the affected region once (so cells never flash the wrong sub-geometry).
    /// Assumes the footprint was gated clear. Returns false if `block` isn't a model
    /// block or any cell is unloaded.
    pub fn place_model_block(&mut self, base: IVec3, block: Block) -> bool {
        self.place_model_block_facing(base, block, block_model::DEFAULT_MODEL_FACING)
    }

    /// Oriented form of [`place_model_block`](Self::place_model_block).
    pub fn place_model_block_facing(&mut self, base: IVec3, block: Block, facing: Facing) -> bool {
        let RenderShape::Model(kind) = block.render_shape() else {
            return false;
        };
        let cells = block_model::oriented_footprint_cells(base, kind, facing);
        // Materialize every footprint cell's section (a multi-block can reach into an
        // all-air, hence absent, section), bailing if any is outside the vertical range,
        // so the whole region is writable before the consistent write below.
        for &(c, _) in &cells {
            if !self.materialize_section_at(c) {
                return false;
            }
        }
        // Write block + offset for every cell first (no remesh yet), so the region is
        // fully consistent before any mesh is rebuilt.
        for &(c, off) in &cells {
            let Some((chunk, lx, ly, lz)) = self.chunk_at_world_mut(c.x, c.y, c.z) else {
                return false;
            };
            chunk.set_block(lx, ly, lz, block);
            if off != [0, 0, 0] {
                chunk.set_model_offset(lx, ly, lz, off);
            }
            chunk.set_model_facing(lx, ly, lz, facing);
            chunk.modified = true;
        }
        let positions: Vec<IVec3> = cells.into_iter().map(|(cell, _)| cell).collect();
        self.refresh_region(&positions);
        true
    }

    /// If `pos` is a bbmodel-block cell, the whole multi-block group: its kind, the
    /// rotated-footprint base, and every footprint cell. `None` for a non-model cell.
    pub fn model_group(&self, pos: IVec3) -> Option<(BlockModelKind, IVec3, Vec<IVec3>)> {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        let RenderShape::Model(kind) = block.render_shape() else {
            return None;
        };
        let off = self.model_offset_at(pos.x, pos.y, pos.z);
        let facing = self.model_facing_at(pos.x, pos.y, pos.z);
        let base = block_model::base_from_cell(pos, kind, off, facing);
        Some((
            kind,
            base,
            Self::model_footprint_cells_facing(base, kind, facing),
        ))
    }

    /// Break the whole multi-block `pos` belongs to: set every footprint cell to air
    /// (clearing its offset) and relight + remesh the region once. Returns the cells
    /// removed (for drops/particles), or `None` if `pos` isn't a model block. The
    /// caller spawns a single drop for the block (the group is one item).
    pub fn remove_model_block(&mut self, pos: IVec3) -> Option<Vec<IVec3>> {
        let (_, _, cells) = self.model_group(pos)?;
        for &c in &cells {
            if let Some((chunk, lx, ly, lz)) = self.chunk_at_world_mut(c.x, c.y, c.z) {
                chunk.set_block(lx, ly, lz, Block::Air); // also clears the offset
                chunk.modified = true;
            }
        }
        self.refresh_region(&cells);
        Some(cells)
    }

    /// Relight + remesh the 3×3 neighbourhood of every cell in `cells` (deduped per
    /// chunk) and announce the changes — the batched tail of [`set_block_world`] for a
    /// multi-cell edit.
    ///
    /// [`set_block_world`]: Self::set_block_world
    pub(super) fn refresh_region(&mut self, cells: &[IVec3]) {
        let mut seen = std::collections::HashSet::new();
        for &c in cells {
            if let Some((pos, _, _, _)) = Self::split_world(c.x, c.y, c.z) {
                if seen.insert(pos) {
                    self.refresh_particle_emitter_index(pos);
                    self.mark_dirty_neighborhood(pos, true);
                }
            }
            // The matching 3×3 relight rides along with each cell's announce.
            self.notify_block_and_neighbors(c.x, c.y, c.z);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};

    const WB: Block = Block::FurnitureWorkbench;

    /// A world with a single empty chunk at (0,0) installed, for placement tests.
    fn world_with_empty_chunk() -> World {
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn placing_a_multiblock_fills_its_whole_footprint_with_offsets() {
        let mut w = world_with_empty_chunk();
        let origin = IVec3::new(5, 64, 5);
        assert!(w.model_footprint_clear(origin, BlockModelKind::FurnitureWorkbench));
        assert!(w.place_model_block(origin, WB));

        // Every occupied cell holds the block id, and the group resolves back to it.
        let (kind, found_origin, cells) = w.model_group(origin).expect("a model group");
        assert_eq!(kind, BlockModelKind::FurnitureWorkbench);
        assert_eq!(found_origin, origin);
        assert_eq!(cells.len(), 4, "the 2×2×1 workbench fills four cells");
        for &c in &cells {
            assert_eq!(Block::from_id(w.chunk_block(c.x, c.y, c.z)), WB, "{c:?}");
            // A non-zero authored cell knows its offset; querying from it finds the same base.
            assert_eq!(w.model_group(c).unwrap().1, origin);
        }
        // The far corner (origin + 1x + 1y) carries a non-zero offset.
        assert_eq!(
            w.model_offset_at(origin.x + 1, origin.y + 1, origin.z),
            [1, 1, 0]
        );
        // Each cell has its own cell-local collision (per-cell split, not the whole box).
        assert!(!w
            .collision_boxes_at(origin.x, origin.y, origin.z)
            .is_empty());
    }

    #[test]
    fn oriented_multiblock_places_from_front_left_anchor() {
        let mut w = world_with_empty_chunk();
        let anchor = IVec3::new(5, 64, 5);
        let base = block_model::base_from_front_left_anchor(
            anchor,
            BlockModelKind::FurnitureWorkbench,
            Facing::North,
        );
        assert_eq!(
            base,
            IVec3::new(4, 64, 5),
            "facing north puts the front-left workbench cell at the clicked anchor"
        );
        assert!(w.model_footprint_clear_facing(
            base,
            BlockModelKind::FurnitureWorkbench,
            Facing::North
        ));
        assert!(w.place_model_block_facing(base, WB, Facing::North));

        let cells = World::model_footprint_cells_facing(
            base,
            BlockModelKind::FurnitureWorkbench,
            Facing::North,
        );
        assert!(cells.contains(&anchor));
        assert!(cells.contains(&(anchor + IVec3::new(-1, 0, 0))));
        assert!(cells.contains(&(anchor + IVec3::new(0, 1, 0))));
        assert!(cells.contains(&(anchor + IVec3::new(-1, 1, 0))));
        for c in cells {
            assert_eq!(Block::from_id(w.chunk_block(c.x, c.y, c.z)), WB);
            assert_eq!(w.model_facing_at(c.x, c.y, c.z), Facing::North);
        }
    }

    #[test]
    fn placement_is_gated_on_the_whole_footprint_being_clear() {
        let mut w = world_with_empty_chunk();
        let origin = IVec3::new(5, 64, 5);
        // Block one of the footprint cells (the +x neighbour) with stone.
        w.set_block_world(origin.x + 1, origin.y, origin.z, Block::Stone);
        assert!(
            !w.model_footprint_clear(origin, BlockModelKind::FurnitureWorkbench),
            "an occupied footprint cell must fail the gate"
        );
    }

    #[test]
    fn breaking_any_cell_removes_the_whole_group() {
        let mut w = world_with_empty_chunk();
        let origin = IVec3::new(5, 64, 5);
        assert!(w.place_model_block(origin, WB));
        // Break from a non-zero authored cell — the whole group must clear.
        let removed = w
            .remove_model_block(origin + IVec3::new(1, 1, 0))
            .expect("removes a model group");
        assert_eq!(removed.len(), 4);
        for c in removed {
            assert_eq!(
                Block::from_id(w.chunk_block(c.x, c.y, c.z)),
                Block::Air,
                "{c:?}"
            );
            assert_eq!(
                w.model_offset_at(c.x, c.y, c.z),
                [0, 0, 0],
                "offset cleared"
            );
        }
    }
}
