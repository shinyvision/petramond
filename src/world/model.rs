#[cfg(test)]
use crate::world::ServerWorld;
use crate::world::WorldData;
use crate::world::{World, WorldSide};
use petramond_math::facing::Facing;
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::block_model::{self, BlockModelKind};

use super::cell_change::{CellChange, ChangeKind};

impl<S: WorldSide> World<S> {
    pub fn place_model_block(&mut self, base: IVec3, block: Block) -> bool {
        self.place_model_block_facing(base, block, block_model::DEFAULT_MODEL_FACING)
    }

    pub fn place_model_block_facing(&mut self, base: IVec3, block: Block, facing: Facing) -> bool {
        let Some(kind) = block.model_kind() else {
            return false;
        };
        let cells = block_model::oriented_footprint_cells(base, kind, facing);
        for &(c, _) in &cells {
            if !self.materialize_section_at(c) {
                return false;
            }
        }
        let mut changes = Vec::with_capacity(cells.len());
        for &(c, off) in &cells {
            let Some((chunk, lx, ly, lz)) = self.data.chunk_at_world_mut(c.x, c.y, c.z) else {
                return false;
            };
            let old = chunk.block(lx, ly, lz);
            chunk.set_block(lx, ly, lz, block);
            if off != [0, 0, 0] {
                chunk.set_model_offset(lx, ly, lz, off);
            }
            chunk.set_model_facing(lx, ly, lz, facing);
            chunk.modified = true;
            changes.push(CellChange::new(c, old, ChangeKind::Place));
        }
        self.apply_cell_changes(&changes);
        true
    }

    pub fn model_group(&self, pos: IVec3) -> Option<(BlockModelKind, IVec3, Vec<IVec3>)> {
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        let kind = block.model_kind()?;
        let off = self.data.model_offset_at(pos.x, pos.y, pos.z);
        let facing = self.data.model_facing_at(pos.x, pos.y, pos.z);
        let base = block_model::base_from_cell(pos, kind, off, facing);
        Some((
            kind,
            base,
            WorldData::model_footprint_cells_facing(base, kind, facing),
        ))
    }

    pub fn swap_block(&mut self, pos: IVec3, new_block: Block) -> bool {
        let current = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        if current.model_kind().is_some() {
            self.swap_model_block(pos, new_block)
        } else if new_block.model_kind().is_some() {
            false
        } else {
            self.swap_block_skin(pos, new_block)
        }
    }

    pub fn swap_model_block(&mut self, pos: IVec3, new_block: Block) -> bool {
        let Some((_, base, cells)) = self.model_group(pos) else {
            return false;
        };
        let Some(new_kind) = new_block.model_kind() else {
            return false;
        };
        if Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z)) == new_block {
            return true;
        }
        let facing = self.data.model_facing_at(pos.x, pos.y, pos.z);
        let new_cells = block_model::oriented_footprint_cells(base, new_kind, facing);
        if new_cells.len() != cells.len() || new_cells.iter().any(|(c, _)| !cells.contains(c)) {
            return false;
        }
        if new_cells
            .iter()
            .any(|&(c, _)| self.data.chunk_at_world(c.x, c.y, c.z).is_none())
        {
            return false;
        }
        let mut changes = Vec::with_capacity(new_cells.len());
        for &(c, off) in &new_cells {
            let (chunk, lx, ly, lz) = self
                .data
                .chunk_at_world_mut(c.x, c.y, c.z)
                .expect("cell resolution verified above");
            let old = chunk.block(lx, ly, lz);
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
            changes.push(CellChange::new(c, old, ChangeKind::Costume));
        }
        self.apply_cell_changes(&changes);
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
            .any(|&c| self.data.chunk_at_world(c.x, c.y, c.z).is_none())
        {
            return false;
        }
        let has = |c: IVec3, key: &str, want: &[u8]| {
            self.data
                .cell_kv_get(c.x, c.y, c.z, key)
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
            self.cell_kv_set(
                c.x,
                c.y,
                c.z,
                block_model::PARTS_KV_KEY.to_owned(),
                parts.to_le_bytes().to_vec(),
            );
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::chunk::{Chunk, ChunkPos};

    const WB: Block = Block::FurnitureWorkbench;

    fn world_with_empty_chunk() -> ServerWorld {
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn placing_a_multiblock_fills_its_whole_footprint_with_offsets() {
        let mut w = world_with_empty_chunk();
        let origin = IVec3::new(5, 64, 5);
        assert!(w
            .data
            .model_footprint_clear(origin, BlockModelKind::FurnitureWorkbench));
        assert!(w.place_model_block(origin, WB));

        let (kind, found_origin, cells) = w.model_group(origin).expect("a model group");
        assert_eq!(kind, BlockModelKind::FurnitureWorkbench);
        assert_eq!(found_origin, origin);
        assert_eq!(cells.len(), 4, "the 2×2×1 workbench fills four cells");
        for &c in &cells {
            assert_eq!(
                Block::from_id(w.data.chunk_block(c.x, c.y, c.z)),
                WB,
                "{c:?}"
            );
            assert_eq!(w.model_group(c).unwrap().1, origin);
        }
        assert_eq!(
            w.data.model_offset_at(origin.x + 1, origin.y + 1, origin.z),
            [1, 1, 0]
        );
        assert!(!w
            .data
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
        assert!(w.data.model_footprint_clear_facing(
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
            assert_eq!(Block::from_id(w.data.chunk_block(c.x, c.y, c.z)), WB);
            assert_eq!(w.data.model_facing_at(c.x, c.y, c.z), Facing::North);
        }
    }

    #[test]
    fn placement_is_gated_on_the_whole_footprint_being_clear() {
        let mut w = world_with_empty_chunk();
        let origin = IVec3::new(5, 64, 5);
        w.set_block_world(origin.x + 1, origin.y, origin.z, Block::Stone);
        assert!(
            !w.data
                .model_footprint_clear(origin, BlockModelKind::FurnitureWorkbench),
            "an occupied footprint cell must fail the gate"
        );
    }

    #[test]
    fn block_writes_clear_the_cells_mod_kv() {
        let mut w = world_with_empty_chunk();

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
            w.data
                .cell_kv_get(pos.x, pos.y, pos.z, "farm:moisture")
                .is_none(),
            "a replaced block takes its cell KV with it"
        );
        assert_eq!(
            w.data
                .cell_kv_get(neighbour.x, neighbour.y, neighbour.z, "farm:moisture"),
            Some(&[1u8][..]),
            "the neighbour's KV is untouched"
        );

        let origin = IVec3::new(9, 64, 5);
        assert!(w.place_model_block(origin, WB));
        assert!(w.cell_kv_set(
            origin.x,
            origin.y,
            origin.z,
            "kitchen:state".into(),
            vec![1, 2, 3]
        ));
        w.remove_compound(origin).expect("removes the group");
        assert!(
            w.data
                .cell_kv_get(origin.x, origin.y, origin.z, "kitchen:state")
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
        let removed = w
            .remove_compound(origin + IVec3::new(1, 1, 0))
            .expect("removes a model group");
        assert_eq!(removed.len(), 4);
        for c in removed {
            assert_eq!(
                Block::from_id(w.data.chunk_block(c.x, c.y, c.z)),
                Block::Air,
                "{c:?}"
            );
            assert_eq!(
                w.data.model_offset_at(c.x, c.y, c.z),
                [0, 0, 0],
                "offset cleared"
            );
        }
    }

    #[test]
    fn a_parts_mask_covers_every_footprint_cell_and_repairs_a_lost_one() {
        let mut w = world_with_empty_chunk();
        let origin = IVec3::new(5, 64, 5);
        assert!(w.place_model_block(origin, WB));
        let cells = w.model_group(origin).expect("a placed group").2;
        assert!(cells.len() > 1, "fixture: this row is multi-cell");

        assert!(w.set_model_parts(origin, 0b101, None));
        let mask_at = |w: &ServerWorld, c: IVec3| {
            w.data
                .cell_kv_get(c.x, c.y, c.z, block_model::PARTS_KV_KEY)
                .map(<[u8; 4]>::try_from)
                .and_then(Result::ok)
                .map(u32::from_le_bytes)
        };
        for &c in &cells {
            assert_eq!(mask_at(&w, c), Some(0b101), "{c:?}");
        }

        let lost = *cells.last().expect("a cell");
        assert!(w.cell_kv_remove(lost.x, lost.y, lost.z, block_model::PARTS_KV_KEY));
        assert_eq!(mask_at(&w, lost), None, "fixture: the cell is now bare");

        assert!(w.set_model_parts(origin, 0b101, None));
        assert_eq!(
            mask_at(&w, lost),
            Some(0b101),
            "resubmitting the current mask must heal a footprint cell that lost it"
        );
    }
}
