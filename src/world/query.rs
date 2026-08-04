
use super::store::World;

impl World {
    /// Is any terrain CPU light/mesh work still queued or in flight? Tooling uses this
    /// to detect when the background pipeline has settled; renderer upload dirtiness is
    /// tracked separately because headless profilers have no renderer to clear it.
    ///
    /// `light_deferred` members count only while a recheck is outstanding: a
    /// deferred section with no queued recheck is PARKED by design (sealed /
    /// unsettled-forever neighbourhood) and only an external event — target
    /// move, topology change, neighbour landing — can wake it, so it is not
    /// pending work.
    pub fn has_dirty_meshes(&self) -> bool {
        !self.terrain.dirty_meshes.is_empty()
            || (self.role != crate::world::WorldRole::ServerHeadless && self.terrain.vis_dirty)
            || !self.terrain.light_blocked_meshes.is_empty()
            || self.deferred_recheck_needed
            || !self.deferred_rechecks.is_empty()
            || self.light_bakes.has_pending()
            || self.terrain.prediction_terrain.has_pending()
            || self.terrain.mesh_jobs_in_flight > 0
    }
    /// Anything still generating, loading from disk or waiting on an overlay.
    pub fn has_pending_stream_work(&self) -> bool {
        !self.gen.pending.is_empty()
            || !self.gen.pending_sections.is_empty()
            || !self.gen.awaited_overlays.is_empty()
            || !self.gen.pending_overlays.is_empty()
    }
    /// Number of sections queued for (re)mesh — the streaming backlog.
    pub fn dirty_mesh_count(&self) -> usize {
        self.terrain.dirty_meshes.len() + self.terrain.light_blocked_meshes.len()
    }
    /// (deep, visible-deep, hidden-parked) counts — a visibility diagnostic for
    /// streaming/perf tooling.
    pub fn deep_visibility_counts(&self) -> (usize, usize, usize) {
        (
            self.terrain.deep_sections.len(),
            self.terrain.visible_deep.len(),
            self.terrain.hidden_parked.len(),
        )
    }
}



#[cfg(test)]
mod tests {
    use petramond_world::block::Block;
    use petramond_world::chunk::ChunkPos;

    use super::*;

    #[test]
    fn full_spawn_support_rejects_water_leaves_partials_and_unloaded_cells() {
        let mut world = World::new(0, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));

        assert!(!world.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::Grass));
        assert!(world.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::Water));
        assert!(!world.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::OakLeaves));
        assert!(!world.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::OakStairs));
        assert!(!world.block_is_full_spawn_support(8, 63, 8));

        assert!(!world.block_is_full_spawn_support(128, 63, 128));
    }
}
