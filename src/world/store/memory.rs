//! Resident-memory census of a loaded world.
//!
//! Byte accounting for the stores that scale with view distance: section voxel
//! cubes, light cubes, per-column 2D maps, retained CPU meshes and the streaming
//! bookkeeping sets. Shared buffers (`Arc`) are counted ONCE per distinct
//! allocation — a uniform all-air section pointing at the shared cube must not
//! be billed 4 KiB it does not own, or the census would flatter every fix that
//! increases sharing.

use crate::world::{World, WorldSide};
use std::collections::HashSet;


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
    pub fluid_cubes: usize,
    pub fluid_bytes: u64,
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
    /// Worldgen memo entries held (shared memos) and the resident bytes of
    /// every memo generation reads through (see `ServerWorld::worldgen_cache_report`).
    pub worldgen_cache_entries: usize,
    pub worldgen_cache_bytes: u64,
}

impl MemoryCensus {
    pub fn total(&self) -> u64 {
        self.block_bytes
            + self.skylight_bytes
            + self.blocklight_bytes
            + self.fluid_bytes
            + self.section_structs
            + self.sparse_state_bytes
            + self.entity_bytes
            + self.emitter_cell_bytes
            + self.column_bytes
            + self.column_gen_bytes
            + self.mesh_capacity_bytes
            + self.index_bytes
            + self.worldgen_cache_bytes
    }
}

fn map_bytes<K, V>(len: usize) -> u64 {
    // rustc-hash / std hashbrown: one (K,V) plus a control byte per slot, at
    // ~87.5% max load. Close enough to bill an index set honestly.
    ((std::mem::size_of::<K>() + std::mem::size_of::<V>() + 1) as u64) * (len as u64) * 8 / 7
}

impl<S: WorldSide> World<S> {
    /// Where this world's resident bytes are. See [`MemoryCensus`].
    pub fn memory_census(&self) -> MemoryCensus {
        let mut c = MemoryCensus::default();
        if let Some(server) = self.side.server() {
            for memo in server.gen.caches.report() {
                c.worldgen_cache_entries += memo.entries;
                c.worldgen_cache_bytes += memo.bytes;
            }
        }
        let mut seen: HashSet<usize> = HashSet::with_capacity(self.data.sections.len() * 2);
        c.sections = self.data.sections.len();
        c.section_structs =
            (std::mem::size_of::<petramond_world::section::Section>() as u64) * (c.sections as u64);
        for s in self.data.sections.values() {
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
            let (fluid_ptr, fluid_len, sparse, entities, emitters) = s.memory_parts();
            if let Some(p) = fluid_ptr {
                if seen.insert(p) {
                    c.fluid_cubes += 1;
                    c.fluid_bytes += fluid_len as u64;
                }
            }
            c.sparse_state_bytes += sparse;
            c.entity_bytes += entities;
            c.emitter_cell_bytes += emitters;
        }
        c.columns = self.data.columns.len();
        c.column_bytes = (self.data.columns.len() as u64)
            * (std::mem::size_of::<petramond_world::column::Column>() as u64 + 2 * 1024 + 256);
        if let Some(server) = self.side.server() {
            c.column_gen = server.gen.column_gen.len();
            for g in server.gen.column_gen.values() {
                c.column_gen_bytes += g.memory_bytes();
            }
        }
        c.index_bytes = map_bytes::<
            petramond_world::chunk::SectionPos,
            std::sync::Arc<petramond_world::section::Section>,
        >(self.data.sections.len())
            + map_bytes::<petramond_world::chunk::ChunkPos, petramond_world::column::Column>(
                self.data.columns.len(),
            )
            + map_bytes::<petramond_world::chunk::ChunkPos, u64>(
                self.data.column_payload_revisions.len(),
            )
            + map_bytes::<petramond_world::chunk::ChunkPos, u32>(
                self.data.section_column_cys.len(),
            )
            + map_bytes::<petramond_world::chunk::SectionPos, ()>(self.data.light_deferred.len());
        if let Some(replica) = self.side.replica() {
            let t = &replica.terrain;
            for m in t.meshes.values() {
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
            c.index_bytes += map_bytes::<petramond_world::chunk::SectionPos, petramond_mesh::ChunkMesh>(
                t.meshes.len(),
            ) + map_bytes::<petramond_world::chunk::ChunkPos, u32>(t.mesh_column_cys.len())
                + map_bytes::<petramond_world::chunk::ChunkPos, u64>(t.mesh_upload_revisions.len())
                + map_bytes::<petramond_world::chunk::ChunkPos, ()>(t.mesh_columns.len())
                + map_bytes::<petramond_world::chunk::SectionPos, ()>(t.deep_sections.len())
                + map_bytes::<petramond_world::chunk::SectionPos, ()>(t.visible_deep.len())
                + map_bytes::<petramond_world::chunk::SectionPos, ()>(t.hidden_parked.len())
                + map_bytes::<petramond_world::chunk::SectionPos, ()>(t.sealed_parked.len())
                + map_bytes::<petramond_world::chunk::SectionPos, ()>(t.light_blocked_meshes.len())
                + map_bytes::<petramond_world::chunk::ChunkPos, u64>(t.mesh_release_after.len());
        }
        c
    }
}
