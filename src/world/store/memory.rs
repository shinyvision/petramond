//! Resident-memory census of a loaded world.
//!
//! Byte accounting for the stores that scale with view distance: section voxel
//! cubes, light cubes, per-column 2D maps, retained CPU meshes and the streaming
//! bookkeeping sets. Shared buffers (`Arc`) are counted ONCE per distinct
//! allocation — a uniform all-air section pointing at the shared cube must not
//! be billed 4 KiB it does not own, or the census would flatter every fix that
//! increases sharing.

use std::collections::HashSet;

use super::World;

#[derive(Default, Debug, Clone, Copy)]
pub struct MemoryCensus {
    pub sections: usize,
    pub columns: usize,
    /// Distinct block cubes (shared uniform cubes counted once) and their bytes.
    pub block_cubes: usize,
    pub block_bytes: u64,
    pub skylight_cubes: usize,
    pub skylight_bytes: u64,
    pub blocklight_cubes: usize,
    pub blocklight_bytes: u64,
    pub water_cubes: usize,
    pub water_bytes: u64,
    /// `size_of::<Section>()` × sections — the struct bodies themselves.
    pub section_structs: u64,
    pub sparse_state_bytes: u64,
    pub entity_bytes: u64,
    pub emitter_cell_bytes: u64,
    pub column_bytes: u64,
    pub column_gen: usize,
    pub column_gen_bytes: u64,
    /// Retained CPU section meshes (post-upload releases excluded from bytes).
    pub meshes: usize,
    pub meshes_released: usize,
    pub mesh_bytes: u64,
    pub mesh_capacity_bytes: u64,
    /// Streaming/index sets and maps keyed by section or column.
    pub index_bytes: u64,
    /// Per-stream used mesh bytes: opaque v/i, far v/i, transparent v/i,
    /// translucent v/i, model v/i, contact v.
    pub mesh_streams: [u64; 11],
}

impl MemoryCensus {
    pub fn total(&self) -> u64 {
        self.block_bytes
            + self.skylight_bytes
            + self.blocklight_bytes
            + self.water_bytes
            + self.section_structs
            + self.sparse_state_bytes
            + self.entity_bytes
            + self.emitter_cell_bytes
            + self.column_bytes
            + self.column_gen_bytes
            + self.mesh_capacity_bytes
            + self.index_bytes
    }
}

fn map_bytes<K, V>(len: usize) -> u64 {
    // rustc-hash / std hashbrown: one (K,V) plus a control byte per slot, at
    // ~87.5% max load. Close enough to bill an index set honestly.
    ((std::mem::size_of::<K>() + std::mem::size_of::<V>() + 1) as u64) * (len as u64) * 8 / 7
}

impl World {
    /// Where this world's resident bytes are. See [`MemoryCensus`].
    pub fn memory_census(&self) -> MemoryCensus {
        let mut c = MemoryCensus::default();
        let mut seen: HashSet<usize> = HashSet::with_capacity(self.sections.len() * 2);
        c.sections = self.sections.len();
        c.section_structs =
            (std::mem::size_of::<petramond_world::section::Section>() as u64) * (c.sections as u64);
        for s in self.sections.values() {
            let (ptr, bytes) = s.block_cube_heap();
            if seen.insert(ptr) {
                c.block_cubes += 1;
                c.block_bytes += bytes;
            }
            if let Some(sky) = s.skylight_arc() {
                if seen.insert(sky.as_ptr() as usize) {
                    c.skylight_cubes += 1;
                    c.skylight_bytes += sky.len() as u64;
                }
            }
            if let Some(bl) = s.blocklight_arc() {
                if seen.insert(bl.as_ptr() as usize) {
                    c.blocklight_cubes += 1;
                    c.blocklight_bytes +=
                        (bl.len() * std::mem::size_of::<petramond_world::light::LightRgb>()) as u64;
                }
            }
            let (water_ptr, water_len, sparse, entities, emitters) = s.memory_parts();
            if let Some(p) = water_ptr {
                if seen.insert(p) {
                    c.water_cubes += 1;
                    c.water_bytes += water_len as u64;
                }
            }
            c.sparse_state_bytes += sparse;
            c.entity_bytes += entities;
            c.emitter_cell_bytes += emitters;
        }
        c.columns = self.columns.len();
        c.column_bytes = (self.columns.len() as u64)
            * (std::mem::size_of::<petramond_world::column::Column>() as u64 + 2 * 1024 + 256);
        c.column_gen = self.gen.column_gen.len();
        for g in self.gen.column_gen.values() {
            c.column_gen_bytes += g.memory_bytes();
        }
        for m in self.terrain.meshes.values() {
            c.meshes += 1;
            if m.is_released() {
                c.meshes_released += 1;
            }
            let (used, cap) = m.memory_bytes();
            c.mesh_bytes += used;
            c.mesh_capacity_bytes += cap;
            for (dst, src) in c.mesh_streams.iter_mut().zip(m.stream_bytes()) {
                *dst += src;
            }
        }
        c.index_bytes = map_bytes::<
            petramond_world::chunk::SectionPos,
            std::sync::Arc<petramond_world::section::Section>,
        >(self.sections.len())
            + map_bytes::<petramond_world::chunk::ChunkPos, petramond_world::column::Column>(
                self.columns.len(),
            )
            + map_bytes::<petramond_world::chunk::SectionPos, petramond_mesh::ChunkMesh>(
                self.terrain.meshes.len(),
            )
            + map_bytes::<petramond_world::chunk::ChunkPos, u64>(
                self.column_payload_revisions.len(),
            )
            + map_bytes::<petramond_world::chunk::ChunkPos, u32>(
                self.terrain.mesh_column_cys.len(),
            )
            + map_bytes::<petramond_world::chunk::ChunkPos, u32>(
                self.data.section_column_cys.len(),
            )
            + map_bytes::<petramond_world::chunk::ChunkPos, u64>(
                self.terrain.mesh_upload_revisions.len(),
            )
            + map_bytes::<petramond_world::chunk::ChunkPos, ()>(self.terrain.mesh_columns.len())
            + map_bytes::<petramond_world::chunk::SectionPos, ()>(self.terrain.deep_sections.len())
            + map_bytes::<petramond_world::chunk::SectionPos, ()>(self.terrain.visible_deep.len())
            + map_bytes::<petramond_world::chunk::SectionPos, ()>(self.terrain.hidden_parked.len())
            + map_bytes::<petramond_world::chunk::SectionPos, ()>(self.terrain.sealed_parked.len())
            + map_bytes::<petramond_world::chunk::SectionPos, ()>(self.light_deferred.len())
            + map_bytes::<petramond_world::chunk::SectionPos, ()>(
                self.terrain.light_blocked_meshes.len(),
            )
            + map_bytes::<petramond_world::chunk::ChunkPos, u64>(
                self.terrain.mesh_release_after.len(),
            );
        c
    }
}
