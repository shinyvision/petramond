use crate::chunk::{ChunkPos, CHUNK_SY};
use crate::mesh::ChunkMesh;

use super::store::World;

pub trait WorldQuery {
    fn chunk_block(&self, wx: i32, wy: i32, wz: i32) -> u8;
    fn chunk_loaded(&self, cx: i32, cz: i32) -> bool;
}

impl WorldQuery for World {
    fn chunk_block(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        if wy < 0 || wy >= CHUNK_SY as i32 {
            return 0;
        }
        let cx = wx >> 4;
        let cz = wz >> 4;
        let lx = (wx & 0x0F) as usize;
        let lz = (wz & 0x0F) as usize;
        if let Some(c) = self.chunks.get(&ChunkPos::new(cx, cz)) {
            c.block_raw(lx, wy as usize, lz)
        } else {
            0
        }
    }

    fn chunk_loaded(&self, cx: i32, cz: i32) -> bool {
        self.chunks.contains_key(&ChunkPos::new(cx, cz))
    }
}

impl World {
    /// Iterate loaded chunk meshes for rendering (caller culls by camera).
    pub fn iter_meshes(&self) -> impl Iterator<Item = (ChunkPos, &ChunkMesh)> {
        self.meshes.iter().map(|(p, m)| (*p, m))
    }

    pub fn chunk_block(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        WorldQuery::chunk_block(self, wx, wy, wz)
    }

    /// Biome id for the loaded world column at `(wx, wz)`, or `None` if its
    /// owning chunk is not currently loaded.
    pub fn column_biome(&self, wx: i32, wz: i32) -> Option<u8> {
        let cx = wx >> 4;
        let cz = wz >> 4;
        self.chunks
            .get(&ChunkPos::new(cx, cz))
            .map(|c| c.biome_at((wx & 0x0F) as usize, (wz & 0x0F) as usize))
    }

    /// Is the chunk at chunk-coords `(cx, cz)` loaded?
    pub fn chunk_loaded(&self, cx: i32, cz: i32) -> bool {
        WorldQuery::chunk_loaded(self, cx, cz)
    }
}
