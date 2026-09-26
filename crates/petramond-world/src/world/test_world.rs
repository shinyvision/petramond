//! A minimal world for this crate's own tests: [`WorldData`] behind the
//! [`BehaviorWorld`] seam the block behaviours run against.
//!
//! The engine's `World` sequences everything an edit reaches (light, meshes,
//! replication, heightmaps, block-entity records); none of that exists here.
//! An edit writes the cell and re-runs the shape-refinement cascade, because
//! refined connection state is data the queries in this crate decode. Tests
//! built on it assert data-layer rules only.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::block::behavior::{BehaviorWorld, BlockHook};
use crate::block::{Block, ShapeNeighborhood};
use crate::block_state::StairState;
use crate::chunk::{Chunk, ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY};
use crate::mathh::{IVec3, FACE_NEIGHBORS};
use crate::section::{Section, SectionSummary};
use crate::slab::SlabSlot;

use super::data::WorldData;

/// Runaway backstop for one refinement cascade (the engine's own bound).
const REFINE_BUDGET: usize = 4096;

/// [`WorldData`] with raw, cascade-refined edits — see the module doc.
pub(crate) struct TestWorld {
    pub data: WorldData,
}

impl TestWorld {
    /// An empty world: no sections, no columns.
    pub fn new(seed: u32) -> Self {
        Self {
            data: WorldData::new(seed, 1),
        }
    }

    /// An empty world with one column [`Chunk`] installed at its own position.
    pub fn with_chunk(seed: u32, chunk: Chunk) -> Self {
        let mut w = Self::new(seed);
        w.insert_chunk(ChunkPos::new(chunk.cx, chunk.cz), chunk);
        w
    }

    /// Install a whole column [`Chunk`], split into sections + column data the
    /// way the streamer installs a generated column, with the per-section
    /// summaries a fully known column answers for its absent (empty) sections.
    pub fn insert_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        debug_assert_eq!((pos.cx, pos.cz), (chunk.cx, chunk.cz));
        let (column, sections) = crate::column_split::split_generated_column(&chunk);
        self.data.ensure_column(pos);
        self.data.columns.insert(pos, column);
        let mut sums = vec![SectionSummary::Empty; (SECTION_MAX_CY - SECTION_MIN_CY + 1) as usize]
            .into_boxed_slice();
        for (cy, section) in sections {
            sums[(cy - SECTION_MIN_CY) as usize] = section.summary();
            self.data
                .sections
                .insert(SectionPos::new(pos.cx, cy, pos.cz), Arc::new(section));
        }
        self.data.column_summaries.insert(pos, sums);
    }

    /// Install a column of empty (all-air) sections over the whole vertical
    /// range, so an edit anywhere in it lands in a loaded section.
    pub fn insert_empty_column(&mut self, pos: ChunkPos) {
        self.data.ensure_column(pos);
        for cy in WorldData::column_section_range() {
            self.data.sections.insert(
                SectionPos::new(pos.cx, cy, pos.cz),
                Arc::new(Section::new(pos.cx, cy, pos.cz)),
            );
        }
    }

    /// Write `b` at world `(wx, wy, wz)`, materializing an empty section (and
    /// its column) on demand, then refine the neighbourhood. The section's
    /// raw setter clears the cell's per-cell state and mod KV, as an engine
    /// edit does. `false` only outside the vertical range.
    pub fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        let Some((pos, lx, ly, lz)) = WorldData::split_world(wx, wy, wz) else {
            return false;
        };
        self.ensure_section(pos);
        let Some(section) = self.data.section_mut(pos) else {
            return false;
        };
        section.set_block(lx, ly, lz, b);
        section.modified = true;
        self.refine_around(IVec3::new(wx, wy, wz));
        true
    }

    /// Place a single-cell stair with its placed state (the engine's
    /// `place_stair` data writes).
    pub fn place_stair(&mut self, pos: IVec3, block: Block, state: StairState) -> bool {
        if !crate::stair::is_stair(block) || !self.ensure_section_at(pos) {
            return false;
        }
        let Some((section, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        section.set_block(lx, ly, lz, block);
        section.set_stair_state(lx, ly, lz, state);
        section.modified = true;
        self.refine_around(pos);
        true
    }

    /// Place one slab layer into `slot` of `pos` (the engine's
    /// `place_slab_layer` data writes).
    pub fn place_slab_layer(&mut self, pos: IVec3, block: Block, slot: SlabSlot) -> bool {
        if !self.ensure_section_at(pos) {
            return false;
        }
        let Some(next) = self.data.slab_layer_target_state(pos, block, slot) else {
            return false;
        };
        let representative = crate::slab::representative_block(next);
        let Some((section, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        section.set_block(lx, ly, lz, representative);
        section.set_slab_state(lx, ly, lz, next);
        section.modified = true;
        self.refine_around(pos);
        true
    }

    /// Take the blocks the simulation destroyed (natural breaks), in order.
    pub fn take_natural_breaks(&mut self) -> Vec<(IVec3, Block)> {
        std::mem::take(&mut self.data.sim.pending_breaks)
    }

    fn ensure_section(&mut self, pos: SectionPos) {
        if !self.data.sections.contains_key(&pos) {
            self.data.ensure_column(pos.chunk_pos());
            self.data
                .sections
                .insert(pos, Arc::new(Section::new(pos.cx, pos.cy, pos.cz)));
        }
    }

    fn ensure_section_at(&mut self, c: IVec3) -> bool {
        match SectionPos::from_world(c.x, c.y, c.z) {
            Some(pos) => {
                self.ensure_section(pos);
                true
            }
            None => false,
        }
    }

    /// The edit-time refinement cascade over data alone: re-resolve the
    /// refined state of `seed` and its face neighbours, cascading through
    /// every cell whose stored state changed.
    fn refine_around(&mut self, seed: IVec3) {
        let mut queue: VecDeque<IVec3> = VecDeque::with_capacity(8);
        queue.push_back(seed);
        for d in FACE_NEIGHBORS {
            queue.push_back(seed + d);
        }
        let mut budget = REFINE_BUDGET;
        while let Some(p) = queue.pop_front() {
            assert!(budget > 0, "shape refine cascade overran its budget at {p:?}");
            budget -= 1;
            let block = Block::from_id(self.data.chunk_block(p.x, p.y, p.z));
            if !block.shape_refines() {
                continue;
            }
            let k = block.shape_kind_def();
            let Some((c, lx, ly, lz)) = self.data.chunk_at_world(p.x, p.y, p.z) else {
                continue;
            };
            let cur = c.cell_state(lx, ly, lz);
            let next = k
                .sim
                .refine_state(&k.params, &self.data as &dyn ShapeNeighborhood, p, block, cur);
            if next == cur {
                continue;
            }
            if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(p.x, p.y, p.z) {
                c.set_cell_state(lx, ly, lz, next);
            }
            for d in FACE_NEIGHBORS {
                queue.push_back(p + d);
            }
        }
    }
}

impl BehaviorWorld for TestWorld {
    fn data(&self) -> &WorldData {
        &self.data
    }

    fn queue_block_hook(&mut self, hook: BlockHook) {
        self.data.queue_block_hook(hook);
    }

    fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        TestWorld::set_block_world(self, wx, wy, wz, b)
    }

    /// Record the natural break and leave the block's break residue, as the
    /// engine does (it also sweeps block-entity records, which this world
    /// never holds).
    fn break_block_naturally(&mut self, pos: IVec3) {
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        if block == Block::Air {
            return;
        }
        self.data.sim.pending_breaks.push((pos, block));
        let below = Block::from_id(self.data.chunk_block(pos.x, pos.y - 1, pos.z));
        TestWorld::set_block_world(self, pos.x, pos.y, pos.z, block.break_residue(below));
    }
}
