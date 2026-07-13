//! Legacy 16x16x256 voxel column — the worldgen transfer format and the
//! column-era test fixture.
//!
//! Live world storage is the cubic [`crate::section::Section`] (16³), keyed by
//! [`SectionPos`]. [`Chunk`] survives in exactly two roles:
//!
//! - **Worldgen transfer format**: `worldgen::driver::generate_surface` and the
//!   staged pipeline behind `worldgen::generate_chunk` fill a whole column at
//!   once (blocks, water metadata, heightmap, biome — never block entities),
//!   consumed by the worldgen bins/audit tooling (`genmap`, `genparity`,
//!   `genfeature`) and by worldgen parity tests.
//! - **Test fixture**: column-era tests hand-build a `Chunk` and install it via
//!   `World::insert_chunk_for_test`, which splits it into sections like the old
//!   streamer did (`world::stream::split_generated_column`); `mesh`'s legacy
//!   whole-column meshing/skylight paths also still run against it under
//!   `cfg(test)`.

#[cfg(test)]
use std::collections::HashMap;

use crate::block::Block;
#[cfg(test)]
use crate::facing::Facing;
#[cfg(test)]
use crate::furnace::Furnace;

pub const CHUNK_SX: usize = 16;
pub const CHUNK_SZ: usize = 16;
pub const CHUNK_SY: usize = 256;
pub const SECTION_SIZE: usize = 16;
/// Voxels in one cubic section (16×16×16). The unit of the cubic-chunks refactor.
pub const SECTION_VOLUME: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

// --- Cubic-chunk world vertical range ---------------------------------------
// The cubic world spans `WORLD_MIN_Y..WORLD_MAX_Y`. Sea level and the surface
// datum are unchanged (see `SEA_LEVEL`); the extra room below 0 exists so caves
// have somewhere to carve. These are the tunable extents of the section grid.
pub const WORLD_MIN_Y: i32 = -64;
pub const WORLD_MAX_Y: i32 = 256;
/// Lowest / highest section coordinate `cy` (inclusive). `cy` is
/// `wy.div_euclid(16)`, so it is negative below y=0.
pub const SECTION_MIN_CY: i32 = WORLD_MIN_Y / SECTION_SIZE as i32;
pub const SECTION_MAX_CY: i32 = WORLD_MAX_Y / SECTION_SIZE as i32 - 1;

// Matches the reference overworld sea level so the land/water line aligns with the
// reference terrain (offset-0 land sits at ≈63.5, just above the waterline).
pub const SEA_LEVEL: i32 = 63;

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

/// Section-local block index for a cubic section: `x,y,z` each in `0..16`. Layout
/// matches [`idx`] within one section (`y*256 + z*16 + x`), so the same value also
/// serves as the `u16` block-entity key (max 4095, well inside `u16`).
#[inline]
pub fn section_idx(x: usize, y: usize, z: usize) -> usize {
    debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
    (y * SECTION_SIZE * SECTION_SIZE) + (z * SECTION_SIZE) + x
}

/// Inverse of [`section_idx`]: the section-local `(x, y, z)` a linear cell index
/// (equivalently, a sparse block-state `u16` key) refers to.
#[inline]
pub fn section_local(idx: usize) -> (usize, usize, usize) {
    debug_assert!(idx < SECTION_VOLUME);
    (
        idx % SECTION_SIZE,
        idx / (SECTION_SIZE * SECTION_SIZE),
        (idx / SECTION_SIZE) % SECTION_SIZE,
    )
}

/// A legacy voxel column: the worldgen transfer format + column-era test
/// fixture (see the module doc). Blocks stored as `Box<[u8; VOLUME]>`
/// (64 KiB / chunk). Carries only what worldgen produces — blocks, water
/// metadata, heightmap, biome — plus a test-only furnace map for the legacy
/// mesh tests. Live world storage is [`crate::section::Section`].
pub struct Chunk {
    pub cx: i32,
    pub cz: i32,
    blocks: Box<[u8]>,
    /// Per-block water state, parallel to `blocks`, only meaningful where the
    /// block is `Water`. Encodes the flow `falloff` (0 = source/full, 1..=8 =
    /// distance from a source) plus a `FALLING` bit (see `world::water`).
    /// `None` until the column first holds non-source flowing water — generated
    /// water is all-source (meta 0), so worldgen output never allocates it;
    /// only test fixtures with flowing water do.
    water: Option<Box<[u8]>>,
    /// Furnace fixtures `(state, facing)`, keyed by local block index
    /// (`idx(x,y,z)` fits a u16). Worldgen never produces block entities; this
    /// survives solely for the legacy whole-column mesh tests (front texture).
    #[cfg(test)]
    furnaces: HashMap<u16, (Furnace, Facing)>,
    /// Highest non-air Y per (x,z) column for fast surface queries.
    pub heightmap: Box<[u16; CHUNK_SX * CHUNK_SZ]>,
    /// Biome id per (x,z) column (Biome::from_id).
    pub biomes: Box<[u8; CHUNK_SX * CHUNK_SZ]>,
    pub dirty: bool,
    /// Cached skylight (x2 scale), a `16 x 16 x (sky_yhi-sky_ylo+1)` band
    /// indexed like `blocks` but with Y offset by `sky_ylo`. Only the legacy
    /// `mesh::skylight` tests compute/read it.
    #[cfg(test)]
    skylight: Box<[u8]>,
    #[cfg(test)]
    sky_ylo: i32,
    #[cfg(test)]
    sky_yhi: i32,
    /// Set when blocks change; cleared when the skylight band is recomputed.
    pub light_dirty: bool,
    /// Count of blocks in this column that receive random ticks (see
    /// [`Block::has_random_tick`]). Maintained incrementally by every setter and
    /// recomputed on bulk load (`recompute_random_tick_count`).
    random_tick_count: u32,
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
            water: None,
            random_tick_count: 0,
            #[cfg(test)]
            furnaces: HashMap::new(),
            heightmap,
            biomes,
            dirty: true,
            #[cfg(test)]
            skylight: Vec::new().into_boxed_slice(),
            #[cfg(test)]
            sky_ylo: 0,
            #[cfg(test)]
            sky_yhi: 0,
            light_dirty: true,
        }
    }

    /// Skylight (x2 scale) at a local voxel. Above the cached band reads as open
    /// sky, below as dark; an uncomputed band reads as open sky (so a not-yet-lit
    /// chunk renders bright rather than black for the brief moment before its
    /// light is baked).
    #[cfg(test)]
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
    #[cfg(test)]
    pub fn set_skylight(&mut self, band: Box<[u8]>, ylo: i32, yhi: i32) {
        self.skylight = band;
        self.sky_ylo = ylo;
        self.sky_yhi = yhi;
        self.light_dirty = false;
    }

    #[inline]
    fn mark_light_dirty(&mut self) {
        self.light_dirty = true;
    }

    pub fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Block::from_id(self.blocks[idx(x, y, z)])
    }

    pub fn block_raw(&self, x: usize, y: usize, z: usize) -> u8 {
        self.blocks[idx(x, y, z)]
    }

    /// Test-fixture setter with full bookkeeping (heightmap, water meta, random-tick
    /// count). Worldgen writes via [`set_block_raw`](Self::set_block_raw) /
    /// `blocks_slice_mut`.
    #[cfg(test)]
    pub fn set_block(&mut self, x: usize, y: usize, z: usize, b: Block) {
        self.set_block_raw(x, y, z, b.id());
    }

    pub fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        let i = idx(x, y, z);
        let old = self.blocks[i];
        self.blocks[i] = id;
        self.adjust_random_tick_count(old, id);
        self.clear_water_meta(i);
        self.update_heightmap_after_set(x, y, z, id);
        self.dirty = true;
        self.mark_light_dirty();
    }

    /// Water-flow metadata at a local voxel (0 where the cell is not flowing
    /// water or the column has never held flowing water). See `world::water`.
    /// Generated water is all-source (meta 0); only fixtures ever store meta.
    #[cfg(test)]
    #[inline]
    pub fn water_meta(&self, x: usize, y: usize, z: usize) -> u8 {
        match &self.water {
            Some(w) => w[idx(x, y, z)],
            None => 0,
        }
    }

    /// Set a water cell (block + flow meta) WITHOUT marking skylight dirty: water
    /// is transparent and never changes the skylight band, so flow updates only
    /// need a remesh. Marks the chunk mesh-dirty. `meta` is ignored (treated as
    /// 0) when `b` is not water.
    #[cfg(test)]
    pub fn set_water(&mut self, x: usize, y: usize, z: usize, b: Block, meta: u8) {
        let i = idx(x, y, z);
        let id = b.id();
        let old = self.blocks[i];
        self.blocks[i] = id;
        self.adjust_random_tick_count(old, id);
        let meta = if b == Block::Water { meta } else { 0 };
        self.store_water_meta(i, meta);
        self.update_heightmap_after_set(x, y, z, id);
        self.dirty = true;
    }

    #[inline]
    fn clear_water_meta(&mut self, i: usize) {
        if let Some(w) = self.water.as_mut() {
            w[i] = 0;
        }
    }

    #[cfg(test)]
    #[inline]
    fn store_water_meta(&mut self, i: usize, meta: u8) {
        if meta == 0 {
            self.clear_water_meta(i);
            return;
        }
        self.water
            .get_or_insert_with(|| vec![0u8; VOLUME].into_boxed_slice())[i] = meta;
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

    /// Keep [`random_tick_count`](Self::random_tick_count) in step with one cell
    /// changing from `old_id` to `new_id`. Every block setter calls this, so the
    /// per-column gate stays exact through generation and runtime edits alike.
    #[inline]
    fn adjust_random_tick_count(&mut self, old_id: u8, new_id: u8) {
        let was = Block::from_id(old_id).has_random_tick();
        let now = Block::from_id(new_id).has_random_tick();
        match (was, now) {
            (false, true) => self.random_tick_count += 1,
            (true, false) => self.random_tick_count -= 1,
            _ => {}
        }
    }

    /// Recount random-tickable cells from scratch — for a bulk load that fills
    /// `blocks` directly (disk load) instead of going through the setters.
    pub fn recompute_random_tick_count(&mut self) {
        self.random_tick_count = self
            .blocks
            .iter()
            .filter(|&&id| Block::from_id(id).has_random_tick())
            .count() as u32;
    }

    /// Whether this column holds any random-tickable block. The live per-section
    /// gate is `Section::has_random_tickable`; this survives for fixture asserts.
    #[cfg(test)]
    #[inline]
    pub fn has_random_tickable(&self) -> bool {
        self.random_tick_count > 0
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
        self.mark_light_dirty();
    }

    // --- Furnace block-entities (test fixture only) -------------------------------
    //
    // Worldgen never emits block entities, and the live world keeps them on
    // `Section`. These survive solely for the legacy whole-column mesh tests.

    /// Local block-index key for the furnace map (`idx` fits a u16).
    #[cfg(test)]
    #[inline]
    fn block_entity_key(x: usize, y: usize, z: usize) -> u16 {
        idx(x, y, z) as u16
    }

    /// Install a furnace fixture (state + facing) at a local voxel.
    #[cfg(test)]
    pub fn insert_furnace(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        furnace: Furnace,
        facing: Facing,
    ) {
        self.furnaces
            .insert(Self::block_entity_key(x, y, z), (furnace, facing));
    }

    /// Whether the furnace at a local voxel is currently lit — read by the legacy
    /// chunk mesher to pick the burning front texture.
    #[cfg(test)]
    #[inline]
    pub fn is_furnace_lit(&self, x: usize, y: usize, z: usize) -> bool {
        self.furnaces
            .get(&Self::block_entity_key(x, y, z))
            .is_some_and(|(f, _)| f.is_lit())
    }

    /// The facing of the furnace at a local voxel (which way its front points), or
    /// `North` if there is no furnace there.
    #[cfg(test)]
    #[inline]
    pub fn furnace_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.furnaces
            .get(&Self::block_entity_key(x, y, z))
            .map_or(Facing::default(), |(_, facing)| *facing)
    }
}

/// 2D column coordinate `(cx, cz)` — the key for per-column [`crate::column::Column`]
/// data (biome, heightmap) and region-file / entity grouping. One column is a
/// vertical stack of [`SectionPos`].
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

/// 3D section coordinate `(cx, cy, cz)` — the canonical key of the cubic world.
/// `cy` may be negative (`cy = wy.div_euclid(16)`), spanning
/// [`SECTION_MIN_CY`]..=[`SECTION_MAX_CY`].
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

    /// The section containing world `(wx,wy,wz)`, or `None` if `wy` is outside the
    /// world vertical range `WORLD_MIN_Y..WORLD_MAX_Y`.
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

    /// World-space minimum corner of this section.
    #[inline]
    pub fn origin_world(self) -> (i32, i32, i32) {
        (
            self.cx * SECTION_SIZE as i32,
            self.cy * SECTION_SIZE as i32,
            self.cz * SECTION_SIZE as i32,
        )
    }

    /// Whether `cy` is within the world's vertical section range.
    #[inline]
    pub fn cy_in_range(cy: i32) -> bool {
        (SECTION_MIN_CY..=SECTION_MAX_CY).contains(&cy)
    }
}
