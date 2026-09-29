use crate::world::{ReplicaWorld, ServerWorld};

impl ServerWorld {
    pub fn has_pending_stream_work(&self) -> bool {
        !self.side.gen.pending.is_empty()
            || !self.side.gen.pending_sections.is_empty()
            || !self.side.gen.awaited_overlays.is_empty()
            || !self.side.gen.pending_overlays.is_empty()
    }

    pub fn has_pending_light_bakes(&self) -> bool {
        self.light_bakes.has_pending()
            || !self.data.relight_demand.is_empty()
            || !self.data.deferred_rechecks.is_empty()
    }

    /// Light bakes (requested, landed) since the world was created.
    pub fn light_bake_stats(&self) -> (u64, u64) {
        self.light_bakes.stats()
    }
}

impl ReplicaWorld {
    pub fn terrain_presentation_backlog(&self) -> (u32, u32) {
        let mesh = self.side.terrain.dirty_meshes.len()
            + self.side.terrain.mesh_jobs_in_flight
            + self.side.terrain.light_blocked_meshes.len();
        (
            mesh.min(u32::MAX as usize) as u32,
            self.side
                .terrain
                .mesh_upload_dirty_columns
                .len()
                .min(u32::MAX as usize) as u32,
        )
    }

    pub fn has_dirty_meshes(&self) -> bool {
        !self.side.terrain.dirty_meshes.is_empty()
            || self.side.terrain.vis_dirty
            || !self.side.terrain.light_blocked_meshes.is_empty()
            || self.data.deferred_recheck_needed
            || !self.data.deferred_rechecks.is_empty()
            || self.light_bakes.has_pending()
            || self.side.terrain.prediction_terrain.has_pending()
            || self.side.terrain.mesh_jobs_in_flight > 0
    }

    pub fn dirty_mesh_count(&self) -> usize {
        self.side.terrain.dirty_meshes.len() + self.side.terrain.light_blocked_meshes.len()
    }

    pub fn deep_visibility_counts(&self) -> (usize, usize, usize) {
        (
            self.side.terrain.deep_sections.len(),
            self.side.terrain.visible_deep.len(),
            self.side.terrain.hidden_parked.len(),
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
        let mut world = ServerWorld::new(0, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));

        assert!(!world.data.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::Grass));
        assert!(world.data.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::Water));
        assert!(!world.data.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::OakLeaves));
        assert!(!world.data.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::OakStairs));
        assert!(!world.data.block_is_full_spawn_support(8, 63, 8));

        assert!(!world.data.block_is_full_spawn_support(128, 63, 128));
    }
}
