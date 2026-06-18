//! Chunk storage: 16x16x256 voxel column.

use crate::block::Block;

pub const CHUNK_SX: usize = 16;
pub const CHUNK_SZ: usize = 16;
pub const CHUNK_SY: usize = 256;

/// World Y index where chunk column begins (chunks stack vertically too,
/// but we currently use a single 256-tall slab per column).
pub const CHUNK_SY_BASE: i32 = 0;

pub const SEA_LEVEL: i32 = 64;

pub const VOLUME: usize = CHUNK_SX * CHUNK_SY * CHUNK_SZ;

#[inline]
pub fn lx(x: i32) -> usize { (x & 0x0F) as usize }

#[inline]
pub fn lz(z: i32) -> usize { (z & 0x0F) as usize }

#[inline]
pub fn idx(x: usize, y: usize, z: usize) -> usize {
    debug_assert!(x < CHUNK_SX && y < CHUNK_SY && z < CHUNK_SZ);
    (y * CHUNK_SX * CHUNK_SZ) + (z * CHUNK_SX) + x
}

/// A voxel column. Blocks stored as `Box<[u8; VOLUME]>` (256 KiB / chunk).
pub struct Chunk {
    pub cx: i32,
    pub cz: i32,
    blocks: Box<[u8]>,
    /// Highest non-air Y per (x,z) column for fast surface queries.
    pub heightmap: Box<[u16; CHUNK_SX * CHUNK_SZ]>,
    /// Biome id per (x,z) column (Biome::from_id).
    pub biomes: Box<[u8; CHUNK_SX * CHUNK_SZ]>,
    pub dirty: bool,
}

impl Chunk {
    pub fn new(cx: i32, cz: i32) -> Self {
        let blocks = vec![0u8; VOLUME].into_boxed_slice();
        let heightmap = Box::new([0u16; CHUNK_SX * CHUNK_SZ]);
        let biomes = Box::new([0u8; CHUNK_SX * CHUNK_SZ]);
        Self { cx, cz, blocks, heightmap, biomes, dirty: true }
    }

    pub fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Block::from_id(self.blocks[idx(x, y, z)])
    }

    pub fn block_raw(&self, x: usize, y: usize, z: usize) -> u8 {
        self.blocks[idx(x, y, z)]
    }

    pub fn set_block(&mut self, x: usize, y: usize, z: usize, b: Block) {
        let i = idx(x, y, z);
        self.blocks[i] = b.id();
        if b != Block::Air {
            let h = &mut self.heightmap[z * CHUNK_SX + x];
            if (y as u16) > *h { *h = y as u16; }
        }
        self.dirty = true;
    }

    pub fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        let i = idx(x, y, z);
        self.blocks[i] = id;
        if id != 0 {
            let h = &mut self.heightmap[z * CHUNK_SX + x];
            if (y as u16) > *h { *h = y as u16; }
        }
        self.dirty = true;
    }

    pub fn surface_y(&self, x: usize, z: usize) -> i32 {
        self.heightmap[z * CHUNK_SX + x] as i32
    }

    pub fn blocks_slice(&self) -> &[u8] { &self.blocks }
    pub fn blocks_slice_mut(&mut self) -> &mut [u8] { &mut self.blocks }
    pub fn biomes_slice(&self) -> &[u8] { &self.biomes[..] }
    pub fn biomes_slice_mut(&mut self) -> &mut [u8] { &mut self.biomes[..] }
    pub fn biome_at(&self, x: usize, z: usize) -> u8 { self.biomes[z * CHUNK_SX + x] }
    pub fn set_biome(&mut self, x: usize, z: usize, b: u8) { self.biomes[z * CHUNK_SX + x] = b; }

    pub fn chunk_origin_world(&self) -> (i32, i32) {
        (self.cx * CHUNK_SX as i32, self.cz * CHUNK_SZ as i32)
    }

    /// Rebuild heightmap from block data (used when block data arrives fully
    /// from a worker without per-cell update bookkeeping).
    pub fn recompute_heightmap(&mut self) {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let mut h: u16 = 0;
                for y in (0..CHUNK_SY).rev() {
                    if self.blocks[idx(x, y, z)] != 0 {
                        h = y as u16;
                        break;
                    }
                }
                self.heightmap[z * CHUNK_SX + x] = h;
            }
        }
        self.dirty = true;
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChunkPos { pub cx: i32, pub cz: i32 }

impl ChunkPos { pub fn new(cx: i32, cz: i32) -> Self { Self { cx, cz } } }