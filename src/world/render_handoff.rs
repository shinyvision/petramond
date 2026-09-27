use crate::world::ReplicaWorld;
use petramond_mesh::ChunkMesh;
use petramond_world::chunk::{ChunkPos, SectionPos};

use super::side::TerrainRenderState;
use super::store::for_each_column_cy;

pub struct TerrainRenderHandoff<'a> {
    world: &'a mut ReplicaWorld,
}

impl ReplicaWorld {
    pub fn terrain_render_handoff(&mut self) -> TerrainRenderHandoff<'_> {
        TerrainRenderHandoff { world: self }
    }
}

impl TerrainRenderHandoff<'_> {
    fn terrain(&self) -> &TerrainRenderState {
        &self.world.side.terrain
    }

    fn terrain_mut(&mut self) -> &mut TerrainRenderState {
        &mut self.world.side.terrain
    }

    pub fn is_streaming(&self) -> bool {
        self.world.has_dirty_meshes() || !self.terrain().mesh_upload_dirty_columns.is_empty()
    }

    pub fn has_column_mesh(&self, pos: ChunkPos) -> bool {
        self.terrain().column_has_mesh(pos)
    }

    pub fn take_urgent_columns(&mut self) -> Vec<ChunkPos> {
        self.terrain_mut().upload_urgent_columns.drain().collect()
    }

    pub fn for_dirty_columns(&self, f: &mut dyn FnMut(ChunkPos, u64)) {
        let terrain = self.terrain();
        for &column in &terrain.mesh_upload_dirty_columns {
            f(
                column,
                terrain
                    .mesh_upload_revisions
                    .get(&column)
                    .copied()
                    .unwrap_or(0),
            );
        }
    }

    pub fn column_meshes(&self, pos: ChunkPos) -> Vec<(SectionPos, &ChunkMesh)> {
        let terrain = self.terrain();
        let Some(&bits) = terrain.mesh_column_cys.get(&pos) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(bits.count_ones() as usize);
        for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if let Some(mesh) = terrain.meshes.get(&sp) {
                out.push((sp, mesh));
            }
        });
        out
    }

    pub fn needs_repack_remeshes(&mut self, pos: ChunkPos) -> bool {
        let terrain = self.terrain_mut();
        let Some(&bits) = terrain.mesh_column_cys.get(&pos) else {
            return false;
        };
        let mut waiting = false;
        let mut forced = Vec::new();
        for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if terrain.meshes.get(&sp).is_some_and(|m| m.is_released()) {
                waiting = true;
                forced.push(sp);
            }
        });
        for sp in forced {
            if terrain.repack_forced.insert(sp) {
                terrain.dirty_meshes.push(sp);
            }
        }
        waiting
    }

    pub fn request_full_reupload(&mut self) {
        let terrain = &mut self.world.side.terrain;
        terrain
            .mesh_upload_dirty_columns
            .extend(terrain.mesh_columns.iter().copied());
    }

    pub fn mark_column_uploaded(&mut self, pos: ChunkPos) {
        let terrain = self.terrain_mut();
        if let Some(&bits) = terrain.mesh_column_cys.get(&pos) {
            for_each_column_cy(bits, |cy| {
                if let Some(mesh) = terrain.meshes.get_mut(&SectionPos::new(pos.cx, cy, pos.cz)) {
                    mesh.mesh_dirty = false;
                }
            });
        }
        terrain.mesh_upload_dirty_columns.remove(&pos);
        if terrain.mesh_columns.contains(&pos) {
            let release_at = terrain.mesh_pump_frame + super::mesh_queue::MESH_RELEASE_DELAY_FRAMES;
            terrain.mesh_release_after.insert(pos, release_at);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::world::ReplicaWorld;
    use petramond_world::block::Block;
    use petramond_world::chunk::SectionPos;
    use petramond_world::section::Section;

    #[test]
    fn released_meshes_gate_column_repack_and_force_a_remesh() {
        let mut world = ReplicaWorld::new(0, 0);
        let pos = SectionPos::new(0, 0, 0);
        let column = pos.chunk_pos();
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        world.insert_section_for_test(pos, section);
        world.mesh_section_blocking_for_test(pos);
        assert!(!world.side.terrain.meshes[&pos].is_empty());

        world.terrain_render_handoff().mark_column_uploaded(column);
        assert!(world.side.terrain.mesh_release_after.contains_key(&column));

        world.side.terrain.mesh_pump_frame +=
            super::super::mesh_queue::MESH_RELEASE_DELAY_FRAMES * 2;
        world.side.terrain.mesh_pump_frame -= world.side.terrain.mesh_pump_frame % 64;
        world.side.terrain.mesh_pump_frame -= 1;
        world.tick_mesh_budget(0);
        assert!(
            world.side.terrain.meshes[&pos].is_released(),
            "a settled uploaded column should release its CPU buffers"
        );
        assert!(
            !world.side.terrain.meshes[&pos].is_empty(),
            "emptiness must stay truthful after release"
        );

        world.side.terrain.mesh_upload_dirty_columns.insert(column);
        let mut handoff = world.terrain_render_handoff();
        assert!(handoff.needs_repack_remeshes(column));
        assert!(world.side.terrain.repack_forced.contains(&pos));
        assert!(
            world.side.terrain.meshes.contains_key(&pos),
            "gating a repack must never remove the installed mesh"
        );

        for _ in 0..64 {
            if !world.side.terrain.meshes[&pos].is_released() {
                break;
            }
            world.tick_mesh_budget(8);
        }
        assert!(
            !world.side.terrain.meshes[&pos].is_released(),
            "forced remesh did not land"
        );
        assert!(!world.side.terrain.meshes[&pos].is_empty());
        assert!(
            !world.terrain_render_handoff().needs_repack_remeshes(column),
            "a fresh mesh clears the repack gate"
        );
    }

    #[test]
    fn a_full_reupload_requeues_every_meshed_column() {
        let mut world = ReplicaWorld::new(0, 0);
        let pos = SectionPos::new(0, 0, 0);
        let column = pos.chunk_pos();
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        world.insert_section_for_test(pos, section);
        world.mesh_section_blocking_for_test(pos);
        world.terrain_render_handoff().mark_column_uploaded(column);
        assert!(!world
            .side
            .terrain
            .mesh_upload_dirty_columns
            .contains(&column));

        world.terrain_render_handoff().request_full_reupload();
        assert!(world
            .side
            .terrain
            .mesh_upload_dirty_columns
            .contains(&column));
        let mut dirty = Vec::new();
        world
            .terrain_render_handoff()
            .for_dirty_columns(&mut |c, _| dirty.push(c));
        assert_eq!(dirty, [column]);
    }

    #[test]
    fn a_full_reupload_of_an_empty_world_queues_nothing() {
        let mut world = ReplicaWorld::new(0, 0);
        world.terrain_render_handoff().request_full_reupload();
        assert!(world.side.terrain.mesh_upload_dirty_columns.is_empty());
    }
}
