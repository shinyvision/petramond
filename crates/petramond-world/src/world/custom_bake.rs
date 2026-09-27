//! Collision boxes a pack's WASM baked for custom-shape cells.
//!
//! A cell that was never baked, or whose bake trapped, uses its block row's static boxes instead,
//! so placed blocks keep working while the bake is off. Boxes are interned by content, which keeps
//! `World::collision_boxes_at` returning `&'static [Aabb]` without a leak per cell.
//!
//! Data half only. Mutation and orchestration live in the engine crate.

use crate::block::{Aabb, Block};
use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::IVec3;
use crate::world::data::WorldData;
use std::sync::Mutex;

impl WorldData {
    #[inline]
    pub fn custom_shape_boxes(&self, pos: IVec3) -> Option<&'static [Aabb]> {
        self.content.custom_bake.get(&pos).copied()
    }

    pub fn set_custom_bake(&mut self, pos: IVec3, boxes: &[Aabb]) {
        match intern_boxes(boxes) {
            Some(interned) => {
                self.content.custom_bake.insert(pos, interned);
            }
            None => {
                self.content.custom_bake.remove(&pos);
            }
        }
    }

    #[inline]
    pub fn invalidate_custom_bake(&mut self, pos: IVec3) {
        self.content.custom_bake.remove(&pos);
    }

    pub fn bake_cell_input(&self, pos: IVec3, block: Block) -> mod_api::CellInput {
        let n = |dx, dy, dz| {
            mod_api::BlockId(self.physics_block(pos.x + dx, pos.y + dy, pos.z + dz).id())
        };
        let state_key = block.shape_kind_def().params.state_key();
        let read = |dx: i32, dy: i32, dz: i32| {
            state_key.and_then(|k| {
                self.cell_kv_get(pos.x + dx, pos.y + dy, pos.z + dz, k)
                    .map(|v| v.to_vec())
            })
        };
        mod_api::CellInput {
            world_pos: [pos.x, pos.y, pos.z],
            block_id: mod_api::BlockId(block.id()),
            neighbor_ids: [
                n(-1, 0, 0),
                n(1, 0, 0),
                n(0, -1, 0),
                n(0, 1, 0),
                n(0, 0, -1),
                n(0, 0, 1),
            ],
            state: read(0, 0, 0),
            neighbor_states: [
                read(-1, 0, 0),
                read(1, 0, 0),
                read(0, -1, 0),
                read(0, 1, 0),
                read(0, 0, -1),
                read(0, 0, 1),
            ],
        }
    }

    pub fn drain_custom_bake_dirty(&mut self) -> Vec<CustomBakeCell> {
        let mut dirty: Vec<IVec3> = self.content.custom_bake_dirty.drain().collect();
        dirty.sort_by_key(|p| (p.x, p.y, p.z));
        dirty
            .into_iter()
            .filter_map(|pos| {
                let block = crate::block::Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
                if !block.is_custom_shape() {
                    return None;
                }
                Some(CustomBakeCell {
                    pos,
                    shape_kind: block.shape_kind().0,
                    shape_key: block.shape_kind().key(),
                    input: self.bake_cell_input(pos, block),
                })
            })
            .collect()
    }

    #[inline]
    pub fn has_pending_custom_bakes(&self) -> bool {
        !self.content.custom_bake_dirty.is_empty()
    }

    pub fn scan_section_custom_bakes(&mut self, pos: crate::chunk::SectionPos) {
        let Some(section) = self.sections.get(&pos) else {
            return;
        };
        if section.is_empty_air() {
            return;
        }
        let (ox, oy, oz) = pos.origin_world();
        let table = crate::block::BlockTable::current();
        let mut dirty: Vec<IVec3> = Vec::new();
        section.blocks().cells_where(
            |id| table.custom_shape(id),
            |idx| {
                let (lx, ly, lz) = crate::chunk::section_local(idx);
                dirty.push(IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32));
            },
        );
        for p in dirty {
            self.content.custom_bake_dirty.insert(p);
        }
    }

    pub fn remark_state_key_bakes(&mut self, wx: i32, wy: i32, wz: i32, key: &str) {
        if !crate::block::state_key_declared(key) {
            return;
        }
        for (dx, dy, dz) in [
            (0, 0, 0),
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ] {
            let p = IVec3::new(wx + dx, wy + dy, wz + dz);
            let b = Block::from_id(self.chunk_block(p.x, p.y, p.z));
            if b.is_custom_shape() && b.shape_kind_def().params.state_key() == Some(key) {
                self.invalidate_custom_bake(p);
                self.content.custom_bake_dirty.insert(p);
            }
        }
    }

    pub fn evict_custom_bake_section(&mut self, pos: SectionPos) {
        let in_section =
            |p: &IVec3| WorldData::split_world(p.x, p.y, p.z).map(|s| s.0) == Some(pos);
        self.content.custom_bake.retain(|p, _| !in_section(p));
        self.content.custom_bake_dirty.retain(|p| !in_section(p));
    }

    pub fn evict_custom_bake_column(&mut self, pos: ChunkPos) {
        let in_column = |p: &IVec3| {
            ChunkPos::new(
                p.x.div_euclid(crate::chunk::SECTION_SIZE as i32),
                p.z.div_euclid(crate::chunk::SECTION_SIZE as i32),
            ) == pos
        };
        self.content.custom_bake.retain(|p, _| !in_column(p));
        self.content.custom_bake_dirty.retain(|p| !in_column(p));
    }

    pub fn clear_custom_bake(&mut self) {
        self.content.custom_bake.clear();
        self.content.custom_bake_dirty.clear();
    }
}

pub struct CustomBakeCell {
    pub pos: IVec3,
    pub shape_kind: u16,
    pub shape_key: &'static str,
    pub input: mod_api::CellInput,
}

pub fn intern_boxes(boxes: &[Aabb]) -> Option<&'static [Aabb]> {
    let mut intern = INTERN.lock().expect("bake intern lock");
    if let Some(&existing) = intern.iter().find(|&&b| b == boxes) {
        return Some(existing);
    }
    if intern.len() >= INTERN_CAP {
        return None;
    }
    let leaked: &'static [Aabb] = Box::leak(boxes.to_vec().into_boxed_slice());
    intern.push(leaked);
    Some(leaked)
}

static INTERN: Mutex<Vec<&'static [Aabb]>> = Mutex::new(Vec::new());
const INTERN_CAP: usize = 512;
