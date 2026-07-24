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
use crate::mathh::{IVec3, FACE_NEIGHBORS};

use super::store::World;

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
            let k = block.shape_kind_def();
            if !k.refines {
                continue;
            }
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
            if self.replication_capture {
                self.record_block_delta(p.x, p.y, p.z);
            }
            for d in FACE_NEIGHBORS {
                queue.push_back(p + d);
            }
        }
    }
}
