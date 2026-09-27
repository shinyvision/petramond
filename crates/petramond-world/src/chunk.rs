use crate::block::Block;

pub const CHUNK_SX: usize = 16;
pub const CHUNK_SZ: usize = 16;
pub const CHUNK_SY: usize = 256;
pub const SECTION_SIZE: usize = 16;
pub const SECTION_VOLUME: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

pub const WORLD_MIN_Y: i32 = -64;
pub const WORLD_MAX_Y: i32 = 256;
pub const SECTION_MIN_CY: i32 = WORLD_MIN_Y / SECTION_SIZE as i32;
pub const SECTION_MAX_CY: i32 = WORLD_MAX_Y / SECTION_SIZE as i32 - 1;

pub const SEA_LEVEL: i32 = 63;

pub const VOLUME: usize = CHUNK_SX * CHUNK_SY * CHUNK_SZ;

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

#[inline]
pub fn section_idx(x: usize, y: usize, z: usize) -> usize {
    debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
    (y * SECTION_SIZE * SECTION_SIZE) + (z * SECTION_SIZE) + x
}

#[inline]
pub fn section_local(idx: usize) -> (usize, usize, usize) {
    debug_assert!(idx < SECTION_VOLUME);
    (
        idx % SECTION_SIZE,
        idx / (SECTION_SIZE * SECTION_SIZE),
        (idx / SECTION_SIZE) % SECTION_SIZE,
    )
}

pub struct Chunk {
    pub cx: i32,
    pub cz: i32,
    blocks: Box<[u16]>,
    fluid: Option<Box<[u8]>>,
    pub heightmap: Box<[u16; CHUNK_SX * CHUNK_SZ]>,
    pub biomes: Box<[u8; CHUNK_SX * CHUNK_SZ]>,
    pub dirty: bool,
    pub light_dirty: bool,
    random_tick_count: u32,
}

impl Chunk {
    pub fn new(cx: i32, cz: i32) -> Self {
        let blocks = vec![0u16; VOLUME].into_boxed_slice();
        let heightmap = Box::new([0u16; CHUNK_SX * CHUNK_SZ]);
        let biomes = Box::new([0u8; CHUNK_SX * CHUNK_SZ]);
        Self {
            cx,
            cz,
            blocks,
            fluid: None,
            random_tick_count: 0,
            heightmap,
            biomes,
            dirty: true,
            light_dirty: true,
        }
    }

    #[inline]
    fn mark_light_dirty(&mut self) {
        self.light_dirty = true;
    }

    pub fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Block::from_id(self.blocks[idx(x, y, z)])
    }

    pub fn block_raw(&self, x: usize, y: usize, z: usize) -> u16 {
        self.blocks[idx(x, y, z)]
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_block(&mut self, x: usize, y: usize, z: usize, b: Block) {
        self.set_block_raw(x, y, z, b.id());
    }

    pub fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u16) {
        let i = idx(x, y, z);
        let old = self.blocks[i];
        self.blocks[i] = id;
        self.adjust_random_tick_count(old, id);
        self.clear_fluid_meta(i);
        self.update_heightmap_after_set(x, y, z, id);
        self.dirty = true;
        self.mark_light_dirty();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn fluid_meta(&self, x: usize, y: usize, z: usize) -> u8 {
        match &self.fluid {
            Some(w) => w[idx(x, y, z)],
            None => 0,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_fluid(&mut self, x: usize, y: usize, z: usize, b: Block, meta: u8) {
        let i = idx(x, y, z);
        let id = b.id();
        let old = self.blocks[i];
        self.blocks[i] = id;
        self.adjust_random_tick_count(old, id);
        let meta = if b.is_fluid() { meta } else { 0 };
        self.store_fluid_meta(i, meta);
        self.update_heightmap_after_set(x, y, z, id);
        self.dirty = true;
    }

    #[inline]
    fn clear_fluid_meta(&mut self, i: usize) {
        if let Some(w) = self.fluid.as_mut() {
            w[i] = 0;
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    fn store_fluid_meta(&mut self, i: usize, meta: u8) {
        if meta == 0 {
            self.clear_fluid_meta(i);
            return;
        }
        self.fluid
            .get_or_insert_with(|| vec![0u8; VOLUME].into_boxed_slice())[i] = meta;
    }

    fn update_heightmap_after_set(&mut self, x: usize, y: usize, z: usize, id: u16) {
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

    #[inline]
    fn adjust_random_tick_count(&mut self, old_id: u16, new_id: u16) {
        let was = Block::from_id(old_id).has_random_tick();
        let now = Block::from_id(new_id).has_random_tick();
        match (was, now) {
            (false, true) => self.random_tick_count += 1,
            (true, false) => self.random_tick_count -= 1,
            _ => {}
        }
    }

    pub fn recompute_random_tick_count(&mut self) {
        self.random_tick_count = self
            .blocks
            .iter()
            .filter(|&&id| Block::from_id(id).has_random_tick())
            .count() as u32;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[inline]
    pub fn has_random_tickable(&self) -> bool {
        self.random_tick_count > 0
    }

    pub fn blocks_slice(&self) -> &[u16] {
        &self.blocks
    }
    pub fn blocks_slice_mut(&mut self) -> &mut [u16] {
        &mut self.blocks
    }
    pub fn biomes_slice(&self) -> &[u8] {
        &self.biomes[..]
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
        self.mark_light_dirty();
    }
}

#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ChunkPos {
    pub cx: i32,
    pub cz: i32,
}

impl ChunkPos {
    pub fn new(cx: i32, cz: i32) -> Self {
        Self { cx, cz }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SectionPos {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl SectionPos {
    pub const fn new(cx: i32, cy: i32, cz: i32) -> Self {
        Self { cx, cy, cz }
    }

    #[inline]
    pub fn from_world(wx: i32, wy: i32, wz: i32) -> Option<Self> {
        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&wy) {
            return None;
        }
        Some(Self {
            cx: wx >> 4,
            cy: wy.div_euclid(SECTION_SIZE as i32),
            cz: wz >> 4,
        })
    }

    #[inline]
    pub fn chunk_pos(self) -> ChunkPos {
        ChunkPos::new(self.cx, self.cz)
    }

    #[inline]
    pub fn origin_world(self) -> (i32, i32, i32) {
        (
            self.cx * SECTION_SIZE as i32,
            self.cy * SECTION_SIZE as i32,
            self.cz * SECTION_SIZE as i32,
        )
    }

    #[inline]
    pub fn cy_in_range(cy: i32) -> bool {
        (SECTION_MIN_CY..=SECTION_MAX_CY).contains(&cy)
    }
}
