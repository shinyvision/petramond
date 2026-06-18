//! `ProtoChunk` — the scratch buffer worldgen stages write into.
//!
//! Strata P2: `MARGIN = 0`, so a ProtoChunk is byte-for-byte the same column as
//! a `Chunk` and `into_chunk` is a move. P4 widens it to `16 + 2*MARGIN` so
//! features can write into a neighbour border that is cropped on `into_chunk`.

use crate::chunk::Chunk;

/// Border (in blocks) the proto extends beyond the 16×16 chunk footprint on
/// each horizontal side. Zero until P4 introduces cross-chunk features.
pub const MARGIN: i32 = 0;

pub struct ProtoChunk {
    chunk: Chunk,
}

impl ProtoChunk {
    pub fn new(cx: i32, cz: i32) -> Self {
        Self { chunk: Chunk::new(cx, cz) }
    }

    #[inline]
    pub fn chunk_origin_world(&self) -> (i32, i32) {
        self.chunk.chunk_origin_world()
    }

    #[inline]
    pub fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        self.chunk.set_block_raw(x, y, z, id);
    }

    #[inline]
    pub fn block_raw(&self, x: usize, y: usize, z: usize) -> u8 {
        self.chunk.block_raw(x, y, z)
    }

    #[inline]
    pub fn set_biome(&mut self, x: usize, z: usize, id: u8) {
        self.chunk.set_biome(x, z, id);
    }

    /// Crop the margin (a no-op at MARGIN = 0) and emit the finished chunk.
    pub fn into_chunk(self) -> Chunk {
        self.chunk
    }
}
