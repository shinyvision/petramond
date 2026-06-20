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

/// Full skylight on the x2 integer scale used by the mesher (= light level 15).
/// Shared so chunk storage and the flood-fill agree on "open sky".
pub const SKY_FULL: u8 = 30;

#[inline]
pub fn lx(x: i32) -> usize {
    (x & 0x0F) as usize
}

#[inline]
pub fn lz(z: i32) -> usize {
    (z & 0x0F) as usize
}

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
    /// Cached self-contained skylight (x2 scale), a `16 x 16 x (sky_yhi-sky_ylo+1)`
    /// band indexed like `blocks` but with Y offset by `sky_ylo`. Computed from
    /// THIS chunk's blocks only (see `mesh::compute_chunk_skylight`) and reused
    /// across mesh rebuilds until the chunk changes. Empty until first computed.
    pub skylight: Box<[u8]>,
    pub sky_ylo: i32,
    pub sky_yhi: i32,
    /// Set when blocks change; cleared when the skylight band is recomputed.
    pub light_dirty: bool,
}

impl Chunk {
    pub fn new(cx: i32, cz: i32) -> Self {
        let blocks = vec![0u8; VOLUME].into_boxed_slice();
        let heightmap = Box::new([0u16; CHUNK_SX * CHUNK_SZ]);
        let biomes = Box::new([0u8; CHUNK_SX * CHUNK_SZ]);
        Self {
            cx,
            cz,
            blocks,
            heightmap,
            biomes,
            dirty: true,
            skylight: Vec::new().into_boxed_slice(),
            sky_ylo: 0,
            sky_yhi: 0,
            light_dirty: true,
        }
    }

    /// Skylight (x2 scale) at a local voxel. Above the cached band reads as open
    /// sky, below as dark; an uncomputed band reads as open sky (so a not-yet-lit
    /// chunk renders bright rather than black for the brief moment before its
    /// light is baked).
    #[inline]
    pub fn skylight_at(&self, x: usize, y: i32, z: usize) -> u8 {
        if self.skylight.is_empty() || y > self.sky_yhi {
            return SKY_FULL;
        }
        if y < self.sky_ylo {
            return 0;
        }
        let ay = y - self.sky_ylo;
        self.skylight[((ay * CHUNK_SZ as i32 + z as i32) * CHUNK_SX as i32 + x as i32) as usize]
    }

    /// Install a freshly computed skylight band and clear the dirty flag.
    pub fn set_skylight(&mut self, band: Box<[u8]>, ylo: i32, yhi: i32) {
        self.skylight = band;
        self.sky_ylo = ylo;
        self.sky_yhi = yhi;
        self.light_dirty = false;
    }

    pub fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Block::from_id(self.blocks[idx(x, y, z)])
    }

    pub fn block_raw(&self, x: usize, y: usize, z: usize) -> u8 {
        self.blocks[idx(x, y, z)]
    }

    pub fn set_block(&mut self, x: usize, y: usize, z: usize, b: Block) {
        let i = idx(x, y, z);
        let id = b.id();
        self.blocks[i] = id;
        self.update_heightmap_after_set(x, y, z, id);
        self.dirty = true;
        self.light_dirty = true;
    }

    pub fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        let i = idx(x, y, z);
        self.blocks[i] = id;
        self.update_heightmap_after_set(x, y, z, id);
        self.dirty = true;
        self.light_dirty = true;
    }

    fn update_heightmap_after_set(&mut self, x: usize, y: usize, z: usize, id: u8) {
        let hi = z * CHUNK_SX + x;
        let h = self.heightmap[hi];
        if id != 0 {
            if (y as u16) > h {
                self.heightmap[hi] = y as u16;
            }
            return;
        }
        if (y as u16) != h {
            return;
        }
        let mut next = 0u16;
        for yy in (0..y).rev() {
            if self.blocks[idx(x, yy, z)] != 0 {
                next = yy as u16;
                break;
            }
        }
        self.heightmap[hi] = next;
    }

    pub fn surface_y(&self, x: usize, z: usize) -> i32 {
        self.heightmap[z * CHUNK_SX + x] as i32
    }

    pub fn blocks_slice(&self) -> &[u8] {
        &self.blocks
    }
    pub fn blocks_slice_mut(&mut self) -> &mut [u8] {
        &mut self.blocks
    }
    pub fn biomes_slice(&self) -> &[u8] {
        &self.biomes[..]
    }
    pub fn biomes_slice_mut(&mut self) -> &mut [u8] {
        &mut self.biomes[..]
    }
    pub fn biome_at(&self, x: usize, z: usize) -> u8 {
        self.biomes[z * CHUNK_SX + x]
    }
    pub fn set_biome(&mut self, x: usize, z: usize, b: u8) {
        self.biomes[z * CHUNK_SX + x] = b;
    }

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
        self.light_dirty = true;
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChunkPos {
    pub cx: i32,
    pub cz: i32,
}

impl ChunkPos {
    pub fn new(cx: i32, cz: i32) -> Self {
        Self { cx, cz }
    }
}
