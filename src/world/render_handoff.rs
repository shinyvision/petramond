use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_mesh::ChunkMesh;

use super::World;

pub struct TerrainRenderHandoff<'a> {
    world: &'a mut World,
}

impl World {
    pub fn terrain_render_handoff(&mut self) -> TerrainRenderHandoff<'_> {
        TerrainRenderHandoff { world: self }
    }
}

impl TerrainRenderHandoff<'_> {
    /// Whether terrain still has streaming, meshing or upload work in flight —
    /// the "not settled" signal behind idle-only frame work.
    pub fn is_streaming(&self) -> bool {
        self.world.has_dirty_meshes()
            || !self.world.terrain.mesh_upload_dirty_columns.is_empty()
            || self.world.has_pending_stream_work()
    }

    pub fn has_column_mesh(&self, pos: ChunkPos) -> bool {
        self.world.column_has_mesh(pos)
    }

    /// Columns a synchronous click presentation just installed meshes into,
    /// drained. The caller's upload scheduler should upload these without
    /// waiting out its quiet-gate coalescing.
    pub fn take_urgent_columns(&mut self) -> Vec<ChunkPos> {
        self.world
            .terrain
            .upload_urgent_columns
            .drain()
            .collect()
    }

    pub fn for_dirty_columns(&self, f: &mut dyn FnMut(ChunkPos, u64)) {
        for &column in &self.world.terrain.mesh_upload_dirty_columns {
            f(
                column,
                self.world
                    .terrain
                    .mesh_upload_revisions
                    .get(&column)
                    .copied()
                    .unwrap_or(0),
            );
        }
    }

    pub fn column_meshes(&self, pos: ChunkPos) -> Vec<(SectionPos, &ChunkMesh)> {
        let Some(&bits) = self.world.terrain.mesh_column_cys.get(&pos) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(bits.count_ones() as usize);
        World::for_each_mesh_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if let Some(mesh) = self.world.terrain.meshes.get(&sp) {
                out.push((sp, mesh));
            }
        });
        out
    }

    /// A packed-column rebuild needs every section's CPU geometry, but a settled
    /// column may have released it (`release_settled_column_meshes`). When any
    /// section mesh in `pos` is released, queue a forced remesh for each and
    /// return true: the caller must skip this column's upload (leaving it
    /// upload-dirty) until the fresh meshes land. The installed GPU column keeps
    /// drawing meanwhile, so the cost is latency on the repack, never a hole.
    pub fn needs_repack_remeshes(&mut self, pos: ChunkPos) -> bool {
        let Some(&bits) = self.world.terrain.mesh_column_cys.get(&pos) else {
            return false;
        };
        let mut waiting = false;
        let mut forced = Vec::new();
        World::for_each_mesh_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if self
                .world
                .terrain
                .meshes
                .get(&sp)
                .is_some_and(|m| m.is_released())
            {
                waiting = true;
                forced.push(sp);
            }
        });
        for sp in forced {
            // Newly forced sections enter the dirty queue; already-forced ones
            // are somewhere in the pipeline (queued, light-blocked, or in flight).
            if self.world.terrain.repack_forced.insert(sp) {
                self.world.terrain.dirty_meshes.push(sp);
            }
        }
        waiting
    }

    pub fn mark_column_uploaded(&mut self, pos: ChunkPos) {
        if let Some(&bits) = self.world.terrain.mesh_column_cys.get(&pos) {
            World::for_each_mesh_cy(bits, |cy| {
                if let Some(mesh) = self
                    .world
                    .terrain
                    .meshes
                    .get_mut(&SectionPos::new(pos.cx, cy, pos.cz))
                {
                    mesh.mesh_dirty = false;
                }
            });
        }
        self.world.terrain.mesh_upload_dirty_columns.remove(&pos);
        if self.world.terrain.mesh_columns.contains(&pos) {
            self.world.terrain.mesh_release_after.insert(
                pos,
                self.world.terrain.mesh_pump_frame + super::mesh_queue::MESH_RELEASE_DELAY_FRAMES,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};
    use petramond_world::block::Block;
    use petramond_world::section::Section;
    use crate::world::store::World;
    use petramond_world::chunk::SectionPos;

    /// The CPU-release contract: a settled column frees its mesh buffers, a later
    /// repack refuses to upload from released meshes (no silent geometry loss) and
    /// instead forces a remesh; the installed mesh entry is never removed while
    /// the remesh is pending.
    #[test]
    fn released_meshes_gate_column_repack_and_force_a_remesh() {
        let mut world = World::new(0, 0);
        let pos = SectionPos::new(0, 0, 0);
        let column = pos.chunk_pos();
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        world.insert_section_for_test(pos, section);
        world.mesh_section_blocking_for_test(pos);
        assert!(!world.terrain.meshes[&pos].is_empty());

        world.terrain_render_handoff().mark_column_uploaded(column);
        assert!(world.terrain.mesh_release_after.contains_key(&column));

        // Fast-forward past the quiet window onto a sweep frame.
        world.terrain.mesh_pump_frame += super::super::mesh_queue::MESH_RELEASE_DELAY_FRAMES * 2;
        world.terrain.mesh_pump_frame -= world.terrain.mesh_pump_frame % 64;
        world.terrain.mesh_pump_frame -= 1;
        world.tick_mesh_budget(0);
        assert!(
            world.terrain.meshes[&pos].is_released(),
            "a settled uploaded column should release its CPU buffers"
        );
        assert!(
            !world.terrain.meshes[&pos].is_empty(),
            "emptiness must stay truthful after release"
        );

        // A repack request against released meshes must gate the upload and force
        // a remesh rather than packing without the section's geometry.
        world.terrain.mesh_upload_dirty_columns.insert(column);
        let mut handoff = world.terrain_render_handoff();
        assert!(handoff.needs_repack_remeshes(column));
        assert!(world.terrain.repack_forced.contains(&pos));
        assert!(
            world.terrain.meshes.contains_key(&pos),
            "gating a repack must never remove the installed mesh"
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while world.terrain.meshes[&pos].is_released() {
            world.tick_mesh_budget(8);
            assert!(Instant::now() < deadline, "forced remesh did not land");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!world.terrain.meshes[&pos].is_empty());
        assert!(
            !world.terrain_render_handoff().needs_repack_remeshes(column),
            "a fresh mesh clears the repack gate"
        );
    }
}
