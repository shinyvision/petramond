//! `ProtoChunk` — the scratch buffer worldgen stages write into.
//!
//! Strata P2: `MARGIN = 0`, so a ProtoChunk is byte-for-byte the same column as
//! a `Chunk` and `into_chunk` is a move. P4 widens it to `16 + 2*MARGIN` so
//! features can write into a neighbour border that is cropped on `into_chunk`.

use crate::chunk::Chunk;

/// Border (in blocks) considered around a chunk for cross-chunk feature
/// placement: the driver derives feature origins in `[-MARGIN, 16+MARGIN)` so a
/// tree rooted in a neighbour can write its overlapping voxels into this chunk.
///
/// Feature writes are clipped to the chunk's own `[0,16)` (see `FeatureCtx`), so
/// no wider buffer is needed: an in-chunk write only ever reads in-chunk cells,
/// and a feature whose footprint <= MARGIN is materialised identically by every
/// chunk that owns part of it (seam-consistent, no double-placement). Features
/// with footprint > MARGIN (the big oak, ~7) clip at the margin exactly as they
/// clipped at the chunk edge before — an accepted, unchanged limitation.
pub const MARGIN: i32 = 3;

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
