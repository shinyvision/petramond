//! bbmodel blocks at the world level: position-aware collision/selection, multi-cell
//! placement gating, and the footprint group for breaking.
//!
//! A bbmodel block's collision and selection are PER CELL — a multi-block (the workbench
//! is 2×2×1) splits its shape across its footprint, and a cell's shape depends on its
//! authored offset plus placed facing, which only the world knows (the chunk model maps).
//! So the per-cell queries live here, over the chunk-owned placement metadata, while
//! [`Block`]'s own (position-less) accessors answer the authored-origin cell. See
//! [`petramond_world::block_model`].

use crate::world::WorldData;
use petramond_world::block::Block;
use petramond_world::block_model::{self, BlockModelKind};
use petramond_math::facing::Facing;
use petramond_math::math::IVec3;

use super::store::{SkyCoverChange, World};


impl World {
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
        let Some(kind) = block.model_kind() else {
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
        let kind = block.model_kind()?;
        let off = self.model_offset_at(pos.x, pos.y, pos.z);
        let facing = self.model_facing_at(pos.x, pos.y, pos.z);
        let base = block_model::base_from_cell(pos, kind, off, facing);
        Some((
            kind,
            base,
            WorldData::model_footprint_cells_facing(base, kind, facing),
        ))
    }

    /// Swap the whole multi-block group at `pos` to `new_block` — another model
    /// block whose ORIENTED FOOTPRINT matches the current group's exactly (the
    /// lit/unlit machine pair sharing one authored model). Block ids are
    /// rewritten in place and the model offsets + facing re-recorded; the
    /// anchor-keyed container, entity facings, and mod cell KV are untouched,
    /// so a cooking machine keeps its slots and state across the flip. The
    /// region relights + remeshes once — an emission difference between the two
    /// rows re-floods exactly like the furnace's lit flip. Returns `false`
    /// when `pos` is not a model-group cell, `new_block` is not a model block,
    /// or the footprints differ (nothing is written).
    pub fn swap_model_block(&mut self, pos: IVec3, new_block: Block) -> bool {
        let Some((_, base, cells)) = self.model_group(pos) else {
            return false;
        };
        let Some(new_kind) = new_block.model_kind() else {
            return false;
        };
        if Block::from_id(self.chunk_block(pos.x, pos.y, pos.z)) == new_block {
            return true; // already there — an idempotent no-op
        }
        let facing = self.model_facing_at(pos.x, pos.y, pos.z);
        let new_cells = block_model::oriented_footprint_cells(base, new_kind, facing);
        // Compare the actual occupied cell sets, not just the declared boxes: a
        // non-rectangular footprint's occupancy comes from the geometry split.
        if new_cells.len() != cells.len() || new_cells.iter().any(|(c, _)| !cells.contains(c)) {
            return false;
        }
        // All-or-nothing: a group may straddle sections, so verify every cell
        // resolves BEFORE the first write — bailing mid-loop would leave a
        // half-swapped group with a stale mesh.
        if new_cells
            .iter()
            .any(|&(c, _)| self.chunk_at_world(c.x, c.y, c.z).is_none())
        {
            return false;
        }
        for &(c, off) in &new_cells {
            let (chunk, lx, ly, lz) = self
                .chunk_at_world_mut(c.x, c.y, c.z)
                .expect("cell resolution verified above");
            // `set_block` clears the cell's model state AND its mod cell KV
            // (per-cell state dies with the block) — a swap is the same placed
            // machine changing costume, so both are carried across explicitly.
            let kv = chunk.cell_kv_take(lx, ly, lz);
            chunk.set_block(lx, ly, lz, new_block);
            if off != [0, 0, 0] {
                chunk.set_model_offset(lx, ly, lz, off);
            }
            chunk.set_model_facing(lx, ly, lz, facing);
            if let Some(kv) = kv {
                chunk.cell_kv_restore(lx, ly, lz, kv);
            }
            chunk.modified = true;
        }
        let positions: Vec<IVec3> = new_cells.into_iter().map(|(cell, _)| cell).collect();
        self.refresh_region(&positions);
        true
    }

    /// Set the PER-INSTANCE presentation state of the model block at `pos`:
    /// which of its row's optional `parts` are visible, and the tint its row's
    /// `tint_parts` multiply by. `false` = `pos` is not a model block, or a
    /// footprint cell is unloaded.
    ///
    /// It writes the whole footprint because the mesher reads the mask from
    /// the cell it is meshing — a group straddles sections, so a mask stored
    /// only at the anchor would be invisible to every other section. A mod
    /// addresses the machine by any of its cells, exactly like `container_set`.
    ///
    /// RENDER ONLY: collision, selection and lighting stay the row's, so a
    /// machine's hitbox never changes under the player mid-cast.
    pub fn set_model_parts(&mut self, pos: IVec3, parts: u32, tint: Option<[u8; 3]>) -> bool {
        let Some((_, _, cells)) = self.model_group(pos) else {
            return false;
        };
        if cells
            .iter()
            .any(|&c| self.chunk_at_world(c.x, c.y, c.z).is_none())
        {
            return false;
        }
        // Resubmitting the same picture is the obvious idiom — a machine
        // compares against what the world CURRENTLY shows so a visual that
        // drifts heals itself — and every cell of this write queues a re-mesh
        // and a relight, so an idempotent call must not cost one.
        //
        // EVERY cell is checked, not just the addressed one. `cell_kv_set`
        // below refuses a write racing an in-flight saved overlay, so a
        // footprint straddling sections can come out half written; a check on
        // the anchor alone then answers "already correct" forever after, and
        // the other cells show the wrong parts until the mask happens to
        // change value. Self-healing that only heals the cell you asked about
        // is the bug it was built to prevent.
        let has = |c: IVec3, key: &str, want: &[u8]| {
            self.cell_kv_get(c.x, c.y, c.z, key)
                .is_some_and(|v| v == want)
        };
        let unchanged = cells.iter().all(|&c| {
            has(c, block_model::PARTS_KV_KEY, &parts.to_le_bytes())
                && tint.is_none_or(|rgb| has(c, petramond_world::block::TINT_KV_KEY, &rgb))
        });
        if unchanged {
            return true;
        }
        for &c in &cells {
            // Through the WORLD accessor, not the section's: it refuses a write
            // racing an in-flight saved overlay, and a footprint spans sections
            // the addressed cell's finality says nothing about.
            self.cell_kv_set(
                c.x,
                c.y,
                c.z,
                block_model::PARTS_KV_KEY.to_owned(),
                parts.to_le_bytes().to_vec(),
            );
            // Only WRITE the dye key, never clear it: it is shared with the dye
            // system, so `tint: None` must mean "I am not tinting" rather than
            // "erase whatever colour this cell had".
            if let Some(rgb) = tint {
                self.cell_kv_set(
                    c.x,
                    c.y,
                    c.z,
                    petramond_world::block::TINT_KV_KEY.to_owned(),
                    rgb.to_vec(),
                );
            }
        }
        // No `refresh_region` here, deliberately. Each `cell_kv_set` above
        // already queued the meshes that sample its cell (the mask is a
        // mesh-feeding key) and re-marked any custom bake reading it. What
        // `refresh_region` adds on top — a relight ball and a shape re-resolve
        // per cell — is block-CHANGE work, and this call changes no block: its
        // whole contract is that collision, selection and lighting stay the
        // row's.
        true
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

    /// Relight + remesh every section each cell in `cells` can influence and
    /// announce the changes — the batched tail of [`set_block_world`] for a
    /// multi-cell edit.
    ///
    /// [`set_block_world`]: Self::set_block_world
    pub(super) fn refresh_region(&mut self, cells: &[IVec3]) {
        let mut seen = std::collections::HashSet::new();
        // Keyed per world column: consecutive same-column height updates chain
        // (old → mid → new) and merge into one envelope; distinct columns keep
        // their own exact segment for the distance-bounded invalidation.
        let mut sky_changed: std::collections::HashMap<(i32, i32), SkyCoverChange> =
            std::collections::HashMap::new();
        for &c in cells {
            let block = Block::from_id(self.chunk_block(c.x, c.y, c.z));
            if let Some(change) = self.update_column_heights_after_set(c.x, c.y, c.z, block) {
                sky_changed
                    .entry((c.x, c.z))
                    .and_modify(|all| all.merge(change))
                    .or_insert(change);
            }
            if let Some((pos, _, _, _)) = WorldData::split_world(c.x, c.y, c.z) {
                if seen.insert(pos) {
                    self.refresh_particle_emitter_index(pos);
                }
            }
            // A costume swap rewrites the cell's block and model state without
            // dropping its draw set (that is the point of a swap), so the
            // set's cached prim→world transform is re-read here.
            self.refresh_block_draw_placement(c);
            self.queue_dirty_meshes_sampling_cell(c.x, c.y, c.z);
            // The matching relight ball rides along with each cell's announce.
            self.notify_block_and_neighbors(c.x, c.y, c.z);
            // Refined shape state (the cell's own — a placed stair resolves
            // its corner — and the neighbourhood's) re-resolves with the
            // edit, exactly like the `set_block_world` lane.
            self.refine_shape_states_around(c.x, c.y, c.z);
        }
        for ((wx, wz), change) in sky_changed {
            self.mark_sky_cover_edited_at(wx, wz, change);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::chunk::{Chunk, ChunkPos};

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

        let cells = WorldData::model_footprint_cells_facing(
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
    fn block_writes_clear_the_cells_mod_kv() {
        let mut w = world_with_empty_chunk();

        // Plain block replacement: the cell's mod KV dies with the block —
        // air holds no block data. The neighbour's KV is untouched.
        let pos = IVec3::new(5, 64, 5);
        let neighbour = IVec3::new(6, 64, 5);
        w.set_block_world(pos.x, pos.y, pos.z, Block::Stone);
        w.set_block_world(neighbour.x, neighbour.y, neighbour.z, Block::Stone);
        assert!(w.cell_kv_set(pos.x, pos.y, pos.z, "farm:moisture".into(), vec![9]));
        assert!(w.cell_kv_set(
            neighbour.x,
            neighbour.y,
            neighbour.z,
            "farm:moisture".into(),
            vec![1]
        ));
        w.set_block_world(pos.x, pos.y, pos.z, Block::Air);
        assert!(
            w.cell_kv_get(pos.x, pos.y, pos.z, "farm:moisture")
                .is_none(),
            "a replaced block takes its cell KV with it"
        );
        assert_eq!(
            w.cell_kv_get(neighbour.x, neighbour.y, neighbour.z, "farm:moisture"),
            Some(&[1u8][..]),
            "the neighbour's KV is untouched"
        );

        // Breaking a model group clears the anchor's KV too (a broken
        // machine's burn state must not haunt the next one placed there).
        let origin = IVec3::new(9, 64, 5);
        assert!(w.place_model_block(origin, WB));
        assert!(w.cell_kv_set(
            origin.x,
            origin.y,
            origin.z,
            "kitchen:state".into(),
            vec![1, 2, 3]
        ));
        w.remove_model_block(origin).expect("removes the group");
        assert!(
            w.cell_kv_get(origin.x, origin.y, origin.z, "kitchen:state")
                .is_none(),
            "breaking a model block clears its anchor's cell KV"
        );
    }

    #[test]
    fn swap_model_block_guards_shape_and_footprint() {
        let mut w = world_with_empty_chunk();
        let origin = IVec3::new(5, 64, 5);
        assert!(w.place_model_block(origin, WB));
        assert!(
            !w.swap_model_block(origin, Block::Stone),
            "a non-model target refuses"
        );
        assert!(
            !w.swap_model_block(origin, Block::Bed),
            "a footprint-mismatched model refuses"
        );
        assert!(
            w.swap_model_block(origin, WB),
            "swapping to the current block is an idempotent success"
        );
        let (_, base, cells) = w.model_group(origin).expect("the group survives");
        assert_eq!(base, origin);
        assert_eq!(cells.len(), 4, "nothing about the footprint changed");
        assert!(
            !w.swap_model_block(origin + IVec3::new(0, 3, 0), WB),
            "a non-model cell refuses"
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

    /// A parts mask covers the WHOLE footprint, and resubmitting the same mask
    /// REPAIRS a cell that lost it.
    ///
    /// The mesher reads the mask from the cell it is meshing, and `cell_kv_set`
    /// refuses a write racing an in-flight saved overlay — so a footprint
    /// straddling sections can end up half written. A machine resubmits its
    /// current mask every tick precisely so that heals; an idempotence check
    /// that only consults the addressed cell answers "already correct" forever
    /// and the rest of the machine stays wrong.
    #[test]
    fn a_parts_mask_covers_every_footprint_cell_and_repairs_a_lost_one() {
        let mut w = world_with_empty_chunk();
        let origin = IVec3::new(5, 64, 5);
        assert!(w.place_model_block(origin, WB));
        let cells = w.model_group(origin).expect("a placed group").2;
        assert!(cells.len() > 1, "fixture: this row is multi-cell");

        assert!(w.set_model_parts(origin, 0b101, None));
        let mask_at = |w: &World, c: IVec3| {
            w.cell_kv_get(c.x, c.y, c.z, block_model::PARTS_KV_KEY)
                .map(<[u8; 4]>::try_from)
                .and_then(Result::ok)
                .map(u32::from_le_bytes)
        };
        for &c in &cells {
            assert_eq!(mask_at(&w, c), Some(0b101), "{c:?}");
        }

        // A cell loses its mask (a refused write, a stale saved overlay).
        let lost = *cells.last().expect("a cell");
        assert!(w.cell_kv_remove(lost.x, lost.y, lost.z, block_model::PARTS_KV_KEY));
        assert_eq!(mask_at(&w, lost), None, "fixture: the cell is now bare");

        // The SAME mask again — the machine's every-tick resubmission.
        assert!(w.set_model_parts(origin, 0b101, None));
        assert_eq!(
            mask_at(&w, lost),
            Some(0b101),
            "resubmitting the current mask must heal a footprint cell that lost it"
        );
    }
}
