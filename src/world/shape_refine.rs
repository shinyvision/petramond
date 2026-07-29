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
//!
//! Determinism: `refine_state` is a pure function of the neighbourhood and
//! runs identically on the server and on the client replica's predicted
//! edits (the replica IS a `World`, and its edit paths call the same hook).
//! Authoritative deltas ship the server's refined bytes; the drain re-read
//! plus the cascade's own delta capture cover every changed cell.

use std::collections::VecDeque;

use crate::block::{Block, ShapeNeighborhood};
use crate::chunk::{section_idx, section_local, SectionPos, SECTION_SIZE};
use crate::mathh::{IVec3, FACE_NEIGHBORS};

use super::store::{World, WorldRole};

/// Runaway backstop for one cascade. Real cascades touch a handful of cells
/// (the dependency chain is two layers deep — see the module doc).
const REFINE_BUDGET: usize = 4096;

impl World {
    /// Re-resolve the refined shape state of the edited cell and its
    /// neighbourhood, cascading through cells whose stored state changed.
    /// Called from every edit chokepoint (`set_block_world`,
    /// `refresh_region`) on the server AND the predicting replica.
    pub(crate) fn refine_shape_states_around(&mut self, wx: i32, wy: i32, wz: i32) {
        let seed = IVec3::new(wx, wy, wz);
        let mut queue: VecDeque<IVec3> = VecDeque::with_capacity(8);
        queue.push_back(seed);
        for d in FACE_NEIGHBORS {
            queue.push_back(seed + d);
        }
        let mut budget = REFINE_BUDGET;
        while let Some(p) = queue.pop_front() {
            if budget == 0 {
                debug_assert!(false, "shape refine cascade overran its budget at {p:?}");
                break;
            }
            budget -= 1;
            let block = Block::from_id(self.chunk_block(p.x, p.y, p.z));
            // The dense per-id gate, so the seven probes an ordinary edit makes
            // cost seven array reads when nothing shaped is nearby — the `def()`
            // load happens only for a cell that actually refines.
            if !block.shape_refines() {
                continue;
            }
            let k = block.shape_kind_def();
            let Some((c, lx, ly, lz)) = self.chunk_at_world(p.x, p.y, p.z) else {
                continue;
            };
            let cur = c.cell_state(lx, ly, lz);
            let next = k
                .sim
                .refine_state(&k.params, self as &dyn ShapeNeighborhood, p, block, cur);
            if next == cur {
                continue;
            }
            if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(p.x, p.y, p.z) {
                c.set_cell_state(lx, ly, lz, next);
            }
            // The refined shape is drawn geometry: redraw every section whose
            // pad samples the cell, and ship the cell's new state to clients
            // (the replica applies it verbatim — it does not re-refine
            // authoritative deltas).
            self.queue_dirty_meshes_sampling_cell(p.x, p.y, p.z);
            if self.replication.replication_capture {
                self.record_block_delta(p.x, p.y, p.z);
            }
            for d in FACE_NEIGHBORS {
                queue.push_back(p + d);
            }
        }
    }

    /// Re-refine every refining cell of a freshly-LOADED section — the
    /// load-time twin of the edit cascade, called from `note_section_loaded`.
    ///
    /// A section load sets its cells in bulk, bypassing the cascade. That is
    /// sound while the stored bytes were refined when saved — but a kind can
    /// START refining after cells of its block were placed (a pack update
    /// giving an existing row connection semantics, an engine family growing
    /// a refined byte), and those cells would keep stale bytes forever: the
    /// cascade only ever runs on edits. This sweep is what lets refinement
    /// vocabulary EVOLVE over a live world instead of forcing a save wipe.
    ///
    /// Also seeds the refining cells in the six adjacent loaded sections'
    /// FACING boundary layers: a boundary cell refined while its neighbour
    /// section was still unloaded resolved against air, and nothing else
    /// would ever revisit it (its dependency — a plain tagged cube, say —
    /// need not refine itself, so sweeping only the new section can miss it).
    ///
    /// AUTHORITATIVE sides only: the replica ingests the server's refined
    /// bytes verbatim, and re-refining at ingest against a half-streamed
    /// neighbourhood would clobber correct state with locally-wrong answers.
    pub(in crate::world) fn refine_section_shapes(&mut self, pos: SectionPos) {
        if self.role() == WorldRole::ClientReplica {
            return;
        }
        let mut seeds: Vec<IVec3> = Vec::new();
        self.collect_refining_cells(pos, None, &mut seeds);
        let (ox, oy, oz) = pos.origin_world();
        for d in FACE_NEIGHBORS {
            let n = SectionPos::from_world(
                ox + d.x * SECTION_SIZE as i32,
                oy + d.y * SECTION_SIZE as i32,
                oz + d.z * SECTION_SIZE as i32,
            );
            if let Some(n) = n {
                self.collect_refining_cells(n, Some(d), &mut seeds);
            }
        }
        for p in seeds {
            self.refine_shape_states_around(p.x, p.y, p.z);
        }
    }

    /// Push the world positions of `pos`'s cells whose block refines.
    ///
    /// `facing = None` walks the whole section — the freshly-loaded one, every
    /// cell of which arrived in bulk. `facing = Some(d)` walks ONLY the single
    /// 16×16 layer of an already-loaded NEIGHBOUR that touches the section at
    /// `d`: no other cell of it can have resolved against that section's
    /// absence, so the other 3,840 must not be visited. Every section install
    /// on the authoritative side pays this, so the per-cell test is the dense
    /// [`Block::id_refines_shape`] LUT, never a `def()` load.
    fn collect_refining_cells(&self, pos: SectionPos, facing: Option<IVec3>, out: &mut Vec<IVec3>) {
        let Some(section) = self.sections.get(&pos) else {
            return;
        };
        if section.is_empty_air() {
            return;
        }
        let blocks = section.blocks_slice();
        let (ox, oy, oz) = pos.origin_world();
        let mut push = |lx: usize, ly: usize, lz: usize, id: u8| {
            if Block::id_refines_shape(id) {
                out.push(IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32));
            }
        };
        let Some(d) = facing else {
            for (idx, &id) in blocks.iter().enumerate() {
                let (lx, ly, lz) = section_local(idx);
                push(lx, ly, lz, id);
            }
            return;
        };
        // The neighbour lies at `+d`, so the layer of it touching `pos` is its
        // LOW side along that axis when `d` is positive, its high side when
        // negative.
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
                push(lx, ly, lz, blocks[section_idx(lx, ly, lz)]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::block::{Block, CellCodec, ShapeNeighborhood};
    use crate::block_state::{StairHalf, StairState};
    use crate::chunk::SectionPos;
    use crate::facing::Facing;
    use crate::mathh::IVec3;
    use crate::section::Section;
    use crate::world::World;

    /// The oracle: what the cell's own family would refine its state to right
    /// now — the sweep must leave every cell agreeing with it.
    fn refined_now(world: &World, p: IVec3) -> crate::block::ShapeState {
        let block = Block::from_id(world.chunk_block(p.x, p.y, p.z));
        let k = block.shape_kind_def();
        let cur = world.shape_state(p);
        k.sim
            .refine_state(&k.params, world as &dyn ShapeNeighborhood, p, block, cur)
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
        let mut world = World::new(1, 2);
        // Stair A on the +X boundary of section (0,0,0), facing the boundary;
        // stair B just across it, perpendicular — the pair resolves a corner.
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
        // Installed alone, A refines against the unloaded neighbour: a
        // straight stair, and already byte-for-byte the oracle's answer.
        let alone = world.shape_state(a);
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
        // B's install must reach BACK across the boundary: A's join changed.
        assert_eq!(world.shape_state(a), refined_now(&world, a));
        assert_eq!(world.shape_state(b), refined_now(&world, b));
        assert_ne!(
            world.shape_state(a),
            alone,
            "the corner join must actually differ, or this test proves nothing"
        );
    }
}
