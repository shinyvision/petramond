//! The edit-time shape-refinement cascade.
//!
//! A shaped block whose form depends on its neighbours (a fence's arms, a
//! stair's corner join) stores its REFINED state in the unified cell store
//! and re-resolves it here, when an edit reaches it — never on a read. The
//! model is uniform for every shaped block: an edit updates the cell and its
//! neighbours; each touched cell re-resolves through its family's
//! [`ShapeSim::refine_state`]; a cell whose stored state actually CHANGED
//! updates its own neighbours in turn, until the neighbourhood reaches its
//! fixpoint. (A WASM-resolved shape follows the same edit fan-out through the
//! bake pump — `mark_custom_bake_edit` — because its resolver is a guest call
//! that cannot run inline; its cache is its stored form.)
//!
//! Termination: refinement inputs form an ACYCLIC dependency chain — a
//! connection mask reads neighbour blocks, slab fullness, and neighbour
//! stairs' REFINED corners; a stair's corner reads only neighbour stairs'
//! PLACED bits, which no refinement ever changes. So a cascade is at most two
//! layers deep; the budget below is a runaway backstop, not a tuning knob.
//! The same acyclicity makes the fixpoint unique, so the order cells are
//! visited in cannot change what they settle to.
//!
//! Determinism: `refine_state` is a pure function of the neighbourhood and
//! runs identically on the server and on the client replica's predicted
//! edits (the replica shares the `World` edit paths, which call the same hook).
//! Authoritative deltas ship the server's refined bytes; the drain re-read
//! plus the cascade's own delta capture cover every changed cell.

use crate::world::{World, WorldSide};
use rustc_hash::FxHashSet;
use std::collections::VecDeque;

use petramond_math::math::{IVec3, FACE_NEIGHBORS};
use petramond_world::block::{BlockTable, ShapeNeighborhood};
use petramond_world::chunk::{section_idx, section_local, SectionPos, SECTION_SIZE};

const REFINE_BUDGET: usize = 4096;

/// The ids the load scan looks for: refining shapes and custom bakes.
static LOAD_SCAN_IDS: petramond_world::content::Slot<petramond_world::section::IdSet> =
    petramond_world::content::Slot::new(
        "load scan ids",
        &[petramond_world::content::stage::BLOCK_VIEWS],
        |_| {
            Ok(petramond_world::section::IdSet::from_ids(
                petramond_world::block::Block::all()
                    .iter()
                    .filter(|b| b.is_custom_shape() || b.shape_refines())
                    .map(|b| b.id()),
            ))
        },
    );

impl<S: WorldSide> World<S> {
    pub fn refine_shape_states_around(&mut self, wx: i32, wy: i32, wz: i32) {
        self.refine_cells(std::iter::once(IVec3::new(wx, wy, wz)));
    }

    /// One cascade for a whole batch of seeds: every seed and its six face
    /// neighbours re-resolve, and a cell whose stored state changed re-queues
    /// its own neighbours. A cell sits in the queue once at a time, so a
    /// landing section with hundreds of refining cells (a dripstone run, a
    /// camp's stairs) visits each affected cell once rather than once per
    /// seed that touches it, and the reads go through a section cursor that
    /// holds the last section instead of hashing every cell.
    fn refine_cells(&mut self, seeds: impl IntoIterator<Item = IVec3>) {
        let mut queue: VecDeque<IVec3> = VecDeque::new();
        let mut queued: FxHashSet<IVec3> = FxHashSet::default();
        fn push(queue: &mut VecDeque<IVec3>, queued: &mut FxHashSet<IVec3>, p: IVec3) {
            if queued.insert(p) {
                queue.push_back(p);
            }
        }
        for seed in seeds {
            push(&mut queue, &mut queued, seed);
            for d in FACE_NEIGHBORS {
                push(&mut queue, &mut queued, seed + d);
            }
        }
        if queue.is_empty() {
            return;
        }
        let table = BlockTable::current();
        let on_server = self.side.server().is_some();
        let mut budget = queue.len() + REFINE_BUDGET;
        loop {
            let changed = {
                let cursor = self.data.cursor();
                let nb: &dyn ShapeNeighborhood = &cursor;
                loop {
                    let Some(p) = queue.pop_front() else {
                        return;
                    };
                    queued.remove(&p);
                    if budget == 0 {
                        debug_assert!(false, "shape refine cascade overran its budget at {p:?}");
                        return;
                    }
                    budget -= 1;
                    let id = cursor.chunk_block(p);
                    if !table.refines_shape(id) {
                        continue;
                    }
                    let block = table.block(id);
                    let k = block.shape_kind_def();
                    let cur = nb.shape_state(p);
                    let next = k.sim.refine_state(&k.params, nb, p, block, cur);
                    if next != cur {
                        break (p, block, next);
                    }
                }
            };
            let (p, block, next) = changed;
            if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(p.x, p.y, p.z) {
                c.set_cell_state(lx, ly, lz, next);
            }
            self.queue_dirty_meshes_sampling_cell(p.x, p.y, p.z);
            self.record_block_delta(p.x, p.y, p.z);
            if on_server
                && petramond_world::world::light::incremental::light_depends_on_state(block)
            {
                self.relight_cell(p.x, p.y, p.z, Self::LIGHT_REACH);
            }
            for d in FACE_NEIGHBORS {
                push(&mut queue, &mut queued, p + d);
            }
        }
    }

    /// The one pass over a landed section's cells: every custom-shape cell goes to the bake
    /// pump (both sides), and on the authoritative side every refining cell — plus the single
    /// facing layer of each already-loaded neighbour, the only cells of theirs that can have
    /// resolved against this section's absence — seeds one refinement cascade. Every section
    /// install pays this, so the per-cell tests are the dense LUTs, never a `def()` load.
    pub(in crate::world) fn scan_loaded_section(&mut self, pos: SectionPos) {
        let Some(section) = self.data.sections.get(&pos) else {
            return;
        };
        if section.is_empty_air() || !section.may_contain(LOAD_SCAN_IDS.current()) {
            return;
        }
        let on_server = self.side.server().is_some();
        let table = BlockTable::current();
        let blocks = section.blocks();
        let (ox, oy, oz) = pos.origin_world();
        let mut seeds: Vec<IVec3> = Vec::new();
        let mut custom: Vec<IVec3> = Vec::new();
        blocks.cells_where(
            |id| table.custom_shape(id) || (on_server && table.refines_shape(id)),
            |idx| {
                let id = blocks.get(idx);
                let (lx, ly, lz) = section_local(idx);
                let p = IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32);
                if table.custom_shape(id) {
                    custom.push(p);
                }
                if on_server && table.refines_shape(id) {
                    seeds.push(p);
                }
            },
        );
        self.data.mark_custom_bakes_dirty(custom);
        if !on_server {
            return;
        }
        for d in FACE_NEIGHBORS {
            let n = SectionPos::from_world(
                ox + d.x * SECTION_SIZE as i32,
                oy + d.y * SECTION_SIZE as i32,
                oz + d.z * SECTION_SIZE as i32,
            );
            if let Some(n) = n {
                self.collect_facing_layer(n, d, table, &mut seeds);
            }
        }
        self.refine_cells(seeds);
    }

    fn collect_facing_layer(
        &self,
        pos: SectionPos,
        d: IVec3,
        table: BlockTable,
        out: &mut Vec<IVec3>,
    ) {
        let Some(section) = self.data.sections.get(&pos) else {
            return;
        };
        if section.is_empty_air() {
            return;
        }
        let blocks = section.blocks();
        let (ox, oy, oz) = pos.origin_world();
        let fixed = if d.x + d.y + d.z > 0 {
            0
        } else {
            SECTION_SIZE - 1
        };
        for a in 0..SECTION_SIZE {
            for b in 0..SECTION_SIZE {
                let (lx, ly, lz) = if d.x != 0 {
                    (fixed, a, b)
                } else if d.y != 0 {
                    (a, fixed, b)
                } else {
                    (a, b, fixed)
                };
                if table.refines_shape(blocks.get(section_idx(lx, ly, lz))) {
                    out.push(IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::world::ServerWorld;
    use petramond_math::facing::Facing;
    use petramond_math::math::IVec3;
    use petramond_world::block::{Block, CellCodec, ShapeNeighborhood};
    use petramond_world::block_state::{StairHalf, StairState};
    use petramond_world::chunk::SectionPos;
    use petramond_world::section::Section;

    fn refined_now(world: &ServerWorld, p: IVec3) -> petramond_world::block::ShapeState {
        let block = Block::from_id(world.data.chunk_block(p.x, p.y, p.z));
        let k = block.shape_kind_def();
        let cur = world.data.shape_state(p);
        k.sim.refine_state(
            &k.params,
            &world.data as &dyn ShapeNeighborhood,
            p,
            block,
            cur,
        )
    }

    /// Loading a section must RE-REFINE its refining cells (and the facing
    /// boundary layers of already-loaded neighbours): stored refined state
    /// written under an older vocabulary — or resolved while the neighbour
    /// section was still unloaded — heals at load instead of rendering stale
    /// until some unrelated edit happens to touch it. Uses two stairs whose
    /// corner join spans a section boundary, with only their PLACED byte
    /// stored (exactly what a save from before the corner byte existed
    /// holds).
    #[test]
    fn loading_a_section_re_refines_stale_stored_shape_state() {
        let mut world = ServerWorld::new(1, 2);
        let a = IVec3::new(15, 8, 8);
        let b = IVec3::new(16, 8, 8);
        let mut sa = Section::new(0, 0, 0);
        sa.set_block(15, 8, 8, Block::OakStairs);
        sa.set_cell_state(
            15,
            8,
            8,
            StairState::new(Facing::East, StairHalf::Bottom).to_cell(),
        );
        let pa = SectionPos::new(0, 0, 0);
        world.insert_section_for_test(pa, sa);
        let alone = world.data.shape_state(a);
        assert_eq!(alone, refined_now(&world, a), "swept at own install");

        let mut sb = Section::new(1, 0, 0);
        sb.set_block(0, 8, 8, Block::OakStairs);
        sb.set_cell_state(
            0,
            8,
            8,
            StairState::new(Facing::South, StairHalf::Bottom).to_cell(),
        );
        world.insert_section_for_test(SectionPos::new(1, 0, 0), sb);
        assert_eq!(world.data.shape_state(a), refined_now(&world, a));
        assert_eq!(world.data.shape_state(b), refined_now(&world, b));
        assert_ne!(
            world.data.shape_state(a),
            alone,
            "the corner join must actually differ, or this test proves nothing"
        );
    }
}
