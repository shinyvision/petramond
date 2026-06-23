use crate::chunk::{self, ChunkPos, SKY_FULL};
use crate::mesh::ChunkMesh;

use super::store::World;

impl World {
    /// Iterate loaded chunk meshes for rendering (caller culls by camera).
    pub fn iter_meshes(&self) -> impl Iterator<Item = (ChunkPos, &ChunkMesh)> {
        self.meshes.iter().map(|(p, m)| (*p, m))
    }

    /// Iterate loaded chunk meshes mutably — the renderer's GPU-upload path,
    /// which clears each mesh's `mesh_dirty` flag as it uploads. Hands out
    /// `&mut ChunkMesh` per loaded chunk without exposing the backing map.
    pub fn iter_meshes_mut(&mut self) -> impl Iterator<Item = (ChunkPos, &mut ChunkMesh)> {
        self.meshes.iter_mut().map(|(p, m)| (*p, m))
    }

    /// Is a built mesh present for this chunk? Lets the renderer drop GPU
    /// meshes whose CPU mesh is gone without iterating the map.
    pub fn has_mesh(&self, pos: ChunkPos) -> bool {
        self.meshes.contains_key(&pos)
    }

    /// Monotonic counter bumped whenever section visibility changes; the
    /// renderer's section-cull cache keys on it to know when to recompute.
    #[inline]
    pub fn visibility_revision(&self) -> u64 {
        self.visibility_revision
    }

    /// Raw block id at a world voxel. Out of range (above/below the column) or in
    /// an unloaded chunk reads as `0` (air) — the mesh-border air fallback.
    pub fn chunk_block(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.block_raw(lx, ly, lz),
            None => 0,
        }
    }

    /// Water-flow metadata at a world voxel (0 where the cell is not flowing
    /// water or its chunk is unloaded). See `world::water` for the encoding.
    pub fn water_meta_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.water_meta(lx, ly, lz),
            None => 0,
        }
    }

    /// Cached skylight at a world voxel on the x2 scale (`SKY_FULL` = light 15).
    /// Missing chunks read as open sky, matching mesh-border fallback behavior.
    pub fn skylight_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        // Distinct out-of-range fallbacks: open sky ABOVE the column, dark BELOW
        // it (the router collapses both to `None`, so split them back out here).
        if wy < 0 {
            return 0;
        }
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, _, lz)) => c.skylight_at(lx, wy, lz),
            None => SKY_FULL,
        }
    }

    /// Cached skylight converted to the 6-bit packed vertex scale (`0..=63`).
    pub fn skylight6_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        let l = self.skylight_at_world(wx, wy, wz) as u32;
        ((l * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63) as u8
    }

    /// Biome id for the loaded world column at `(wx, wz)`, or `None` if its
    /// owning chunk is not currently loaded.
    pub fn column_biome(&self, wx: i32, wz: i32) -> Option<u8> {
        self.chunks
            .get(&ChunkPos::new(wx >> 4, wz >> 4))
            .map(|c| c.biome_at(chunk::lx(wx), chunk::lz(wz)))
    }

    /// Is the chunk at chunk-coords `(cx, cz)` loaded?
    pub fn chunk_loaded(&self, cx: i32, cz: i32) -> bool {
        self.chunks.contains_key(&ChunkPos::new(cx, cz))
    }
}
