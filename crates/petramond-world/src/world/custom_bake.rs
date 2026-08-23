//! The custom-shape SIM bake cache: per-cell collision boxes a pack's
//! WASM baked, read by the shape's collision facet. A cache MISS (never baked,
//! or a trapped/timed-out bake) falls back to the block row's static collision
//! boxes — the failure policy that keeps placed world data intact while only the
//! bake logic is suspended.
//!
//! Boxes are CONTENT-INTERNED to `'static`, so `World::collision_boxes_at` keeps
//! its `&'static [Aabb]` return without leaking per cell: a gate has two
//! configurations (open / closed), so at most two box sets are ever interned no
//! matter how many gates exist. The intern set is bounded by the shapes'
//! distinct geometries, not by the world.
//! (Data-half queries; the mutation/orchestration half stays in the engine crate.)

use crate::block::{Aabb, Block, ShapeFamily};
use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::IVec3;
use crate::world::data::WorldData;
use std::sync::Mutex;

impl WorldData {
    /// The baked collision boxes for the custom shape at `pos`, or `None` when
    /// the cell has no bake yet (the collision facet then uses the row's static
    /// boxes).
    #[inline]
    pub fn custom_shape_boxes(&self, pos: IVec3) -> Option<&'static [Aabb]> {
        self.content.custom_bake.get(&pos).copied()
    }

    /// Record a custom shape cell's freshly-baked collision boxes. A full intern
    /// set (a runaway per-position bake) drops the cache entry so the cell falls
    /// back to its static boxes instead of leaking an unbounded slice.
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

    /// Drop a cell's bake so the next read re-bakes (or falls back) — the edit
    /// invalidation the block-write lanes call.
    #[inline]
    pub fn invalidate_custom_bake(&mut self, pos: IVec3) {
        self.content.custom_bake.remove(&pos);
    }

    /// The ENTIRE wire input a WASM shape bake receives for one cell: `block`'s
    /// id (the CELL's id may not be written yet — placement bakes the
    /// hypothetical cell), the six neighbour ids, and — for a shape declaring
    /// a `state_key` — the replicated per-cell state of the cell and its six
    /// neighbours, so a stateful shape resolves from STATE (a stair's facing),
    /// not just block ids. ONE builder: the tick's bake pump, the server's
    /// placement-plan gate, and the client place ghost all construct their
    /// inputs here and therefore cannot drift.
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

    /// Take the custom-shape cells that need a (re)bake, each with the neighbour
    /// context a bake reads — cleared, so the host's bake pump processes each
    /// dirty cell once. Cells whose block is no longer a custom shape (broken
    /// since being dirtied) are dropped.
    pub fn drain_custom_bake_dirty(&mut self) -> Vec<CustomBakeCell> {
        // Sort by position so the bake dispatch order is DEFINED and identical
        // on the server and every client replica (C1): the dirty set is a hashed
        // set with no stable order, and a bake that touched instance state would
        // otherwise diverge between the two and desync.
        let mut dirty: Vec<IVec3> = self.content.custom_bake_dirty.drain().collect();
        dirty.sort_by_key(|p| (p.x, p.y, p.z));
        dirty
            .into_iter()
            .filter_map(|pos| {
                let block = crate::block::Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
                if block.shape_family() != crate::block::ShapeFamily::Custom {
                    return None;
                }
                Some(CustomBakeCell {
                    pos,
                    shape_kind: block.shape_kind().0,
                    // The shape's declaration key names the owning pack (namespace).
                    shape_key: block.shape_kind().key(),
                    input: self.bake_cell_input(pos, block),
                })
            })
            .collect()
    }

    /// Whether any custom-shape cell is awaiting a bake — the cheap gate the
    /// tick's bake step checks before building a mod dispatch scope.
    #[inline]
    pub fn has_pending_custom_bakes(&self) -> bool {
        !self.content.custom_bake_dirty.is_empty()
    }

    /// Mark every custom-shape cell in a freshly-LOADED section dirty for
    /// baking. A section load (worldgen, streaming, client ingest, save reload)
    /// sets its cells in BULK, bypassing `mark_custom_bake_edit`, so a chair
    /// restored from disk would never re-bake — it would show the row's static
    /// fallback collision and the cube render forever. This is the load-time
    /// equivalent, called from `note_section_loaded` for every install.
    pub fn scan_section_custom_bakes(&mut self, pos: crate::chunk::SectionPos) {
        let Some(section) = self.sections.get(&pos) else {
            return;
        };
        // An all-air section (the empty sky band, the common case above the
        // surface) can hold no custom shape — skip the id scan entirely.
        if section.is_empty_air() {
            return;
        }
        // The overwhelmingly common non-empty section still holds no custom
        // shape; the scan is a tight LUT loop over the id buffer.
        let (ox, oy, oz) = pos.origin_world();
        let mut dirty: Vec<IVec3> = Vec::new();
        for (idx, id) in section.blocks_iter().enumerate() {
            if Block::from_id(id).shape_family() == ShapeFamily::Custom {
                let (lx, ly, lz) = crate::chunk::section_local(idx);
                dirty.push(IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32));
            }
        }
        for p in dirty {
            self.content.custom_bake_dirty.insert(p);
        }
    }

    /// A cell-KV write to `key` landed: re-bake every stateful custom shape in
    /// the cell's neighbourhood that resolves from this state key, so a shape
    /// reading a neighbour's state (a stair's corner from an adjacent facing)
    /// refreshes. Its SIM bake derives collision from the state too, so the
    /// authoritative side must invalidate, not just the render side. Called by
    /// the host KV write path; the replica's KV ingest re-marks via
    /// `mark_custom_bake_edit`.
    pub fn remark_state_key_bakes(&mut self, wx: i32, wy: i32, wz: i32, key: &str) {
        // Almost every cell-KV write carries a non-state key (a dye use count,
        // an interop row): one registry scan skips the 7-cell world probe.
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
            if b.shape_family() == ShapeFamily::Custom
                && b.shape_kind_def().params.state_key() == Some(key)
            {
                self.invalidate_custom_bake(p);
                self.content.custom_bake_dirty.insert(p);
            }
        }
    }

    /// Drop every cached custom bake (collision + dirty mark) in a section being
    /// evicted — the render-box and light-aperture caches ride the `Section` and
    /// evict with it, but the world-keyed collision map and the dirty set do not,
    /// so a roamed-away section would leave stale collision and churn
    /// `chunk_block` on unloaded coords every bake pump.
    pub fn evict_custom_bake_section(&mut self, pos: SectionPos) {
        let in_section =
            |p: &IVec3| WorldData::split_world(p.x, p.y, p.z).map(|s| s.0) == Some(pos);
        self.content.custom_bake.retain(|p, _| !in_section(p));
        self.content.custom_bake_dirty.retain(|p| !in_section(p));
    }

    /// Drop every cached custom bake in a column being evicted.
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

    /// Drop the whole custom-bake cache (the regen path clears every section).
    pub fn clear_custom_bake(&mut self) {
        self.content.custom_bake.clear();
        self.content.custom_bake_dirty.clear();
    }
}

/// One custom-shape cell awaiting a bake: the routing facts the pumps group
/// dispatches by, plus the `ready wire input`.
pub struct CustomBakeCell {
    pub pos: IVec3,
    pub shape_kind: u8,
    pub shape_key: &'static str,
    pub input: mod_api::CellInput,
}

/// Intern `boxes` to a `'static` slice, reusing an equal set if one exists, or
/// `None` once the intern set is full (the caller then falls back to static
/// boxes rather than leaking without bound).
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

/// Interned `'static` box sets, deduped by content. Small (a handful per custom
/// shape), so a linear scan is cheaper than hashing float boxes.
static INTERN: Mutex<Vec<&'static [Aabb]>> = Mutex::new(Vec::new());
/// Hard cap on distinct interned box sets. A well-behaved shape has a handful of
/// configurations, but a bake keyed on `world_pos` could leak one slice PER CELL
/// forever (the leak is `'static`). Past the cap we refuse to cache new
/// geometry: those cells fall back to their static boxes (the failure policy),
/// which bounds the leak to a fixed, small amount regardless of a hostile bake.
const INTERN_CAP: usize = 512;
