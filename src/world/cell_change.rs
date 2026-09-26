//! The one "cells changed" pipeline.
//!
//! Writers only mutate section data, then describe what they did as
//! [`CellChange`]s; [`World::apply_cell_changes`] runs every consequence of a
//! change — the per-section derived indexes, the retained draw sets, the
//! column heightmaps, remeshing, custom-shape bakes, refined shape state,
//! deep visibility, and the announce (relight, replication delta, nav change
//! log, block updates) — once per batch, in one order. A new derived concern
//! is added here, and every writer (a single edit, a placement, a costume
//! swap, a pasted region) gets it.

use std::collections::HashMap;

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::SectionPos;
use rustc_hash::FxHashSet;

use super::store::SkyCoverChange;
use crate::world::{World, WorldSide};

/// How a write treated the cell it changed, beyond its block id. Decides what
/// happens to a retained draw set there, how far the relight reaches, and
/// whether the nav change log may skip the cell.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(in crate::world) enum ChangeKind {
    /// A plain block write (`set_block_world`): the setter cleared the cell's
    /// per-cell state and mod KV, so a draw set the old block submitted dies
    /// with it. The relight is bounded by the light present at the cell, and
    /// a provably walk-identical swap stays out of the nav log.
    Write,
    /// A structured placement or removal (a door, a model footprint, a stair,
    /// a committed placement plan) that wrote the cell's state explicitly.
    Place,
    /// A pasted cell: its whole contents (state, KV, block entities) were
    /// replaced from a snapshot, so whatever drew there no longer owns it.
    Replace,
    /// The same placed thing changing costume: the owner survives the write,
    /// so its draw set stays and only re-reads its placement.
    Costume,
    /// An authoritative replica delta. Refresh presentation and heightmaps,
    /// while leaving light, shape state, and simulation to the server.
    Remote,
}

/// One changed cell: where, what it held before the write, and how it was
/// written. The new contents are read back from the world.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(in crate::world) struct CellChange {
    pub pos: IVec3,
    pub old: Block,
    pub kind: ChangeKind,
}

impl CellChange {
    pub(in crate::world) fn new(pos: IVec3, old: Block, kind: ChangeKind) -> Self {
        Self { pos, old, kind }
    }
}

impl<S: WorldSide> World<S> {
    /// Run every consequence of `changes` — the single reaction pipeline all
    /// block writers feed. The cells must already hold their new contents.
    pub(in crate::world) fn apply_cell_changes(&mut self, changes: &[CellChange]) {
        if changes.is_empty() {
            return;
        }
        let news: Vec<Block> = changes
            .iter()
            .map(|c| Block::from_id(self.data.chunk_block(c.pos.x, c.pos.y, c.pos.z)))
            .collect();

        // Derived per-section indexes, once per touched section.
        let mut sections = FxHashSet::default();
        for c in changes {
            if let Some(sp) = SectionPos::from_world(c.pos.x, c.pos.y, c.pos.z) {
                if sections.insert(sp) {
                    self.refresh_particle_emitter_index(sp);
                    self.refresh_block_entity_index(sp);
                }
            }
        }

        // Retained draw sets belong to the block that submitted them.
        for (c, &new) in changes.iter().zip(&news) {
            let keeps_owner = match c.kind {
                ChangeKind::Costume => true,
                ChangeKind::Replace => false,
                ChangeKind::Write | ChangeKind::Place => false,
                ChangeKind::Remote => c.old == new,
            };
            if keeps_owner {
                self.refresh_block_draw_placement(c.pos);
            } else {
                self.forget_block_draw(c.pos);
            }
        }

        // Column heightmaps. Consecutive same-column updates chain
        // (old → mid → new) into one envelope; distinct columns keep their own
        // exact segment. The sky-cover relight is marked before the announce
        // below, so the announce sees the region it may relight incrementally
        // exactly as it now stands.
        let mut sky_changed: HashMap<(i32, i32), SkyCoverChange> = HashMap::new();
        for (c, &new) in changes.iter().zip(&news) {
            if let Some(change) =
                self.update_column_heights_after_set(c.pos.x, c.pos.y, c.pos.z, new)
            {
                if c.kind == ChangeKind::Remote {
                    continue;
                }
                sky_changed
                    .entry((c.pos.x, c.pos.z))
                    .and_modify(|all| all.merge(change))
                    .or_insert(change);
            }
        }
        for ((wx, wz), change) in sky_changed {
            self.mark_sky_cover_edited_at(wx, wz, change);
        }

        for (c, &new) in changes.iter().zip(&news) {
            let IVec3 { x, y, z } = c.pos;
            // Re-mesh exactly the sections whose pads sample this cell, so
            // border culling, AO and smooth light stay correct across seams.
            self.queue_dirty_meshes_sampling_cell(x, y, z);
            // A WASM-resolved shape's bake depends on this cell's block and
            // state: drop the cached bakes here and at each face neighbour.
            self.mark_custom_bake_edit(x, y, z, new);
            // Natively refined shapes (fence arms, stair corners) re-resolve
            // now, cascading through neighbours whose stored state changes.
            if c.kind != ChangeKind::Remote {
                self.refine_shape_states_around(x, y, z);
                self.announce_cell_change(c, new);
            }
        }

        // Plane openness may have changed; deep visibility must re-evaluate.
        self.mark_visibility_dirty();
    }

    /// The announce for one change: relight what it can influence, log the
    /// replication delta and the nav change, and queue block updates. A plain
    /// write proves what it can (a light-identical replacement skips the
    /// relight, a solid⇄air edit relights only as far as the light present at
    /// the cell can carry, a walk-identical swap stays out of the nav view);
    /// every other kind announces at full reach.
    fn announce_cell_change(&mut self, c: &CellChange, new: Block) {
        let IVec3 { x, y, z } = c.pos;
        if c.kind != ChangeKind::Write {
            self.notify_block_and_neighbors(x, y, z);
            return;
        }
        let nav_relevant = !super::tick::edit_nav_equivalent(c.old, new);
        if c.old.has_same_light_behavior(new) {
            self.notify_light_equivalent_change_nav(x, y, z, nav_relevant);
        } else {
            let radius = self.edit_light_reach(x, y, z, c.old, new);
            self.notify_block_change_with_light_radius_nav(x, y, z, radius, nav_relevant);
        }
    }
}
