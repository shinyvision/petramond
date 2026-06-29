//! `ProtoChunk` — the scratch buffer worldgen stages write into.
//!
//! It owns the in-chunk scratch `Chunk` used by generation. Terrain fill may
//! write block bytes directly; `into_chunk` rebuilds derived indexes before
//! feature/runtime setters continue on the finished chunk.

use crate::chunk::Chunk;

/// Border (in blocks) considered around a chunk for cross-chunk feature
/// placement: the driver derives feature origins in `[-MARGIN, 16+MARGIN)` so a
/// tree rooted in a neighbour can write its overlapping voxels into this chunk.
///
/// Feature writes are clipped to the chunk's own `[0,16)` (see `FeatureCtx`), so
/// no wider buffer is needed: an in-chunk write only ever reads in-chunk cells,
/// and a feature whose footprint <= MARGIN is materialised identically by every
/// chunk that owns part of it (seam-consistent, no double-placement). MARGIN is
/// sized to the widest feature (redwood branch reach + leaf blob, footprint ~9)
/// so every tree rooted in a neighbour replays seamlessly across chunks.
pub const MARGIN: i32 = 9;

pub struct ProtoChunk {
    chunk: Chunk,
}

impl ProtoChunk {
    pub fn new(cx: i32, cz: i32) -> Self {
        Self {
            chunk: Chunk::new(cx, cz),
        }
    }

    #[inline]
    pub fn cx(&self) -> i32 {
        self.chunk.cx
    }

    #[inline]
    pub fn cz(&self) -> i32 {
        self.chunk.cz
    }

    #[inline]
    pub fn chunk_origin_world(&self) -> (i32, i32) {
        self.chunk.chunk_origin_world()
    }

    /// Raw terrain buffer for the initial density fill only.
    ///
    /// Writes through this slice intentionally skip runtime setter bookkeeping;
    /// `into_chunk` rebuilds the derived indexes before runtime feature edits run.
    #[inline]
    pub(crate) fn terrain_blocks_mut(&mut self) -> &mut [u8] {
        self.chunk.blocks_slice_mut()
    }

    #[inline]
    pub fn set_biome(&mut self, x: usize, z: usize, id: u8) {
        self.chunk.set_biome(x, z, id);
    }

    /// Finalize derived indexes and emit the finished chunk.
    pub fn into_chunk(mut self) -> Chunk {
        self.chunk.recompute_heightmap();
        self.chunk.recompute_random_tick_count();
        self.chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::idx;

    #[test]
    fn into_chunk_rebuilds_indexes_after_raw_terrain_writes() {
        let mut proto = ProtoChunk::new(0, 0);
        proto.terrain_blocks_mut()[idx(3, 17, 5)] = Block::Grass.id();

        let chunk = proto.into_chunk();

        assert_eq!(chunk.block(3, 17, 5), Block::Grass);
        assert_eq!(chunk.surface_y(3, 5), 17);
        assert!(chunk.has_random_tickable());
        assert!(chunk.dirty);
        assert!(chunk.light_dirty);
    }
}
