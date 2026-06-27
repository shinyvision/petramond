//! Chunk storage: 16x16x256 voxel column.

use std::collections::HashMap;

use crate::block::Block;
use crate::chest::Chest;
use crate::door::DoorState;
use crate::furnace::{Facing, Furnace};
use crate::item::{ItemStack, ItemType};
use crate::torch::TorchPlacement;

pub const CHUNK_SX: usize = 16;
pub const CHUNK_SZ: usize = 16;
pub const CHUNK_SY: usize = 256;
pub const SECTION_SIZE: usize = 16;
pub const SECTION_COUNT: usize = CHUNK_SY / SECTION_SIZE;

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
    /// Per-block water state, parallel to `blocks`, only meaningful where the
    /// block is `Water`. Encodes the flow `falloff` (0 = source/full, 1..=8 =
    /// distance from a source) plus a `FALLING` bit (see `world::water`).
    /// `None` until the column first holds non-source flowing water, so still
    /// oceans/rivers (all-source, meta 0) never pay the extra 64 KiB.
    water: Option<Box<[u8]>>,
    /// Furnace block-entities in this chunk, keyed by local block index
    /// (`idx(x,y,z)` fits a u16 — max 65535). A furnace never moves, so it's owned
    /// outright by its chunk: it ticks here, persists in this chunk's save record,
    /// and the mesher reads its lit state locally. Empty for the common chunk.
    furnaces: HashMap<u16, Furnace>,
    /// Chest block-entities in this chunk, keyed by local block index like
    /// [`furnaces`](Self::furnaces). Like a furnace a chest never moves, so it's
    /// owned outright by its chunk: it persists in this chunk's save record and the
    /// renderer reads its contents/facing locally. Empty for the common chunk.
    chests: HashMap<u16, Chest>,
    /// Torch orientations in this chunk, keyed by local block index like the
    /// furnace/chest maps. A torch's only per-instance state is how it is mounted
    /// (floor vs which wall), which the mesher and selection outline read locally
    /// and the save record persists. Empty for the common chunk.
    torches: HashMap<u16, TorchPlacement>,
    /// Multi-cell bbmodel block occupancy, keyed by local block index like the other
    /// per-chunk maps. Stores the authored footprint offset for every cell whose offset
    /// is not `[0,0,0]`; the authored-origin cell and single-cell models default to zero.
    /// The mesher reads it with `model_facings` to know which authored cell a voxel is
    /// (which cubes to render), and break/collision use both to find the rotated
    /// footprint. Empty for the common chunk. See `crate::block_model`.
    model_cells: HashMap<u16, [u8; 3]>,
    /// Per-cell facing for placed bbmodel blocks that need orientation. Stored for every
    /// occupied model cell (including the authored-origin cell) so meshing remains
    /// chunk-local even when a multi-cell model crosses a chunk border. Empty for the
    /// common chunk and for old/non-directional model placements.
    model_facings: HashMap<u16, Facing>,
    /// Growth stage (`0..=2`, i.e. the 1st..3rd stage) of each sapling in this chunk,
    /// keyed by local block index like the other per-instance maps. A sapling block with
    /// no entry reads stage `0` (freshly placed), and every block setter clears the entry
    /// (a removed/grown sapling forgets its stage), so the map holds only living saplings
    /// past stage 0. Sparse — empty for the common chunk. See `world::sapling`.
    sapling_stages: HashMap<u16, u8>,
    /// Door state (facing + open + which-half) of each door cell in this chunk, keyed
    /// by local block index like the other per-instance maps. A door spans two stacked
    /// cells, each with its own entry (the upper carries `top = true`). Read by the
    /// dynamic door renderer + the position-aware collision/selection in `world::door`,
    /// and persisted in the save record. Sparse — empty for the common chunk. See
    /// [`crate::door`].
    doors: HashMap<u16, DoorState>,
    /// Highest non-air Y per (x,z) column for fast surface queries.
    pub heightmap: Box<[u16; CHUNK_SX * CHUNK_SZ]>,
    /// Biome id per (x,z) column (Biome::from_id).
    pub biomes: Box<[u8; CHUNK_SX * CHUNK_SZ]>,
    pub dirty: bool,
    /// Set true by runtime edits (block place/break, water flow) and never by
    /// generation, so only player-touched chunks are written to disk.
    pub modified: bool,
    /// Cached skylight (x2 scale), a `16 x 16 x (sky_yhi-sky_ylo+1)` band
    /// indexed like `blocks` but with Y offset by `sky_ylo`. The world bake may
    /// include loaded neighbor chunks so flood light crosses borders, but only
    /// this chunk's band is stored here. Empty until first computed.
    pub skylight: Box<[u8]>,
    pub sky_ylo: i32,
    pub sky_yhi: i32,
    /// Cached block-light (x2 scale) radiated by emitters (torches), flooded by the
    /// same light worker but kept in its OWN band — sized to the emitters and empty
    /// when none are near, so torch-free chunks pay nothing. Unlike skylight it
    /// reads `0` OUTSIDE its band (there is no block light beyond the flood, vs open
    /// sky above the surface). The mesher samples it alongside skylight to brighten
    /// and warm-tint torch-lit surfaces.
    pub blocklight: Box<[u8]>,
    pub block_ylo: i32,
    pub block_yhi: i32,
    /// Set when blocks change; cleared when the skylight band is recomputed.
    pub light_dirty: bool,
    /// Bumped whenever this chunk's cached light needs a new bake. Async light
    /// workers echo this value back so stale results can be discarded.
    pub light_revision: u64,
    /// Count of blocks in this column that receive random ticks (see
    /// [`Block::has_random_tick`]). Maintained incrementally by every setter and
    /// recomputed on bulk load; the simulation skips a whole column when this is
    /// `0`, so the common ocean/desert/cave column pays nothing (`world::tick`).
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
            furnaces: HashMap::new(),
            chests: HashMap::new(),
            torches: HashMap::new(),
            model_cells: HashMap::new(),
            model_facings: HashMap::new(),
            sapling_stages: HashMap::new(),
            doors: HashMap::new(),
            heightmap,
            biomes,
            dirty: true,
            modified: false,
            skylight: Vec::new().into_boxed_slice(),
            sky_ylo: 0,
            sky_yhi: 0,
            blocklight: Vec::new().into_boxed_slice(),
            block_ylo: 0,
            block_yhi: 0,
            light_dirty: true,
            light_revision: 0,
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

    /// Block-light (x2 scale) at a local voxel: `0` outside the computed band (no
    /// block light beyond the flood, and none at all when the band is empty), else
    /// the cached value.
    #[inline]
    pub fn blocklight_at(&self, x: usize, y: i32, z: usize) -> u8 {
        if self.blocklight.is_empty() || y < self.block_ylo || y > self.block_yhi {
            return 0;
        }
        let ay = y - self.block_ylo;
        self.blocklight[((ay * CHUNK_SZ as i32 + z as i32) * CHUNK_SX as i32 + x as i32) as usize]
    }

    /// Install a freshly computed block-light band (paired with `set_skylight` in the
    /// bake-apply path). An empty band means no emitters were near this chunk.
    pub fn set_blocklight(&mut self, band: Box<[u8]>, ylo: i32, yhi: i32) {
        self.blocklight = band;
        self.block_ylo = ylo;
        self.block_yhi = yhi;
    }

    pub fn mark_light_dirty(&mut self) {
        self.light_dirty = true;
        self.light_revision = self.light_revision.wrapping_add(1);
    }

    /// Clone just the terrain data needed by the skylight solver. The cached
    /// light band itself is intentionally dropped to keep worker jobs smaller.
    pub fn snapshot_for_light_bake(&self) -> Self {
        Self {
            cx: self.cx,
            cz: self.cz,
            blocks: self.blocks.clone(),
            random_tick_count: self.random_tick_count,
            // Water meta does not affect skylight (water is transparent), so the
            // bake never reads it -- drop it to keep the snapshot small.
            water: None,
            // Furnaces/chests/torches don't affect skylight either; the bake never
            // needs them.
            furnaces: HashMap::new(),
            chests: HashMap::new(),
            torches: HashMap::new(),
            model_cells: HashMap::new(),
            model_facings: HashMap::new(),
            sapling_stages: HashMap::new(),
            doors: HashMap::new(),
            heightmap: self.heightmap.clone(),
            biomes: self.biomes.clone(),
            dirty: false,
            modified: false,
            skylight: Vec::new().into_boxed_slice(),
            sky_ylo: 0,
            sky_yhi: 0,
            blocklight: Vec::new().into_boxed_slice(),
            block_ylo: 0,
            block_yhi: 0,
            light_dirty: true,
            light_revision: self.light_revision,
        }
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
        let old = self.blocks[i];
        self.blocks[i] = id;
        self.adjust_random_tick_count(old, id);
        self.clear_water_meta(i);
        self.clear_model_cell(i);
        self.clear_sapling_stage(i);
        self.clear_door(i);
        self.update_heightmap_after_set(x, y, z, id);
        self.dirty = true;
        self.mark_light_dirty();
    }

    pub fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        let i = idx(x, y, z);
        let old = self.blocks[i];
        self.blocks[i] = id;
        self.adjust_random_tick_count(old, id);
        self.clear_water_meta(i);
        self.clear_model_cell(i);
        self.clear_sapling_stage(i);
        self.clear_door(i);
        self.update_heightmap_after_set(x, y, z, id);
        self.dirty = true;
        self.mark_light_dirty();
    }

    /// Drop any multi-block occupancy offset stored at local index `i` — the cell's
    /// former occupant (possibly a multi-block cell) is being overwritten. Cheap no-op
    /// for the common chunk (empty maps).
    #[inline]
    fn clear_model_cell(&mut self, i: usize) {
        if !self.model_cells.is_empty() {
            self.model_cells.remove(&(i as u16));
        }
        if !self.model_facings.is_empty() {
            self.model_facings.remove(&(i as u16));
        }
    }

    /// Record cell `(x,y,z)`'s authored offset within its multi-block footprint (called
    /// by `World::place_model_block` for each non-zero offset). The authored-origin cell
    /// is left unstored (it defaults to `[0,0,0]`).
    #[inline]
    pub fn set_model_offset(&mut self, x: usize, y: usize, z: usize, offset: [u8; 3]) {
        self.model_cells
            .insert(Self::block_entity_key(x, y, z), offset);
        self.dirty = true;
    }

    /// The cell's authored offset within its multi-block footprint, or `[0,0,0]` for a
    /// single-cell model block or authored-origin cell. See the `model_cells` field.
    #[inline]
    pub fn model_offset(&self, x: usize, y: usize, z: usize) -> [u8; 3] {
        self.model_cells
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or([0, 0, 0])
    }

    /// Record the facing for an occupied model cell. Unlike offsets, facing is stored
    /// for every oriented cell, including the authored-origin cell.
    #[inline]
    pub fn set_model_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.model_facings
            .insert(Self::block_entity_key(x, y, z), facing);
        self.dirty = true;
    }

    /// The facing of the placed model cell, or the canonical unrotated bbmodel facing
    /// for old/non-oriented placements. bbmodel blocks author their front on -Z.
    #[inline]
    pub fn model_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.model_facings
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or(crate::block_model::DEFAULT_MODEL_FACING)
    }

    /// The chunk's multi-block occupancy map (local index → offset), for persistence.
    #[inline]
    pub fn model_cells(&self) -> &HashMap<u16, [u8; 3]> {
        &self.model_cells
    }

    /// The chunk's model-facing map (local index → facing), for persistence.
    #[inline]
    pub fn model_facings(&self) -> &HashMap<u16, Facing> {
        &self.model_facings
    }

    // --- Sapling growth stage ---------------------------------------------------

    /// Growth stage (`0..=2`, i.e. the 1st..3rd stage) of the sapling at a local
    /// voxel — `0` when no stage is recorded (a freshly placed sapling, or any
    /// non-sapling cell). See `world::sapling`.
    #[inline]
    pub fn sapling_stage(&self, x: usize, y: usize, z: usize) -> u8 {
        self.sapling_stages
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or(0)
    }

    /// Record a sapling's growth `stage` at a local voxel (stage `0` removes the
    /// entry, since absence reads as `0`). Marks the chunk modified so the stage
    /// persists. Does NOT change the block id — advancing a stage leaves the
    /// sapling block in place (only its metadata moves).
    pub fn set_sapling_stage(&mut self, x: usize, y: usize, z: usize, stage: u8) {
        let key = Self::block_entity_key(x, y, z);
        if stage == 0 {
            self.sapling_stages.remove(&key);
        } else {
            self.sapling_stages.insert(key, stage);
        }
        self.modified = true;
    }

    /// Forget a sapling's growth stage at local index `i` — its cell is being
    /// overwritten (broken, or grown into a log). Cheap no-op for the common chunk
    /// (empty map), mirroring [`clear_model_cell`](Self::clear_model_cell).
    #[inline]
    fn clear_sapling_stage(&mut self, i: usize) {
        if !self.sapling_stages.is_empty() {
            self.sapling_stages.remove(&(i as u16));
        }
    }

    /// The sapling-stage map, for saving (keyed by local block index).
    #[inline]
    pub fn sapling_stages(&self) -> &HashMap<u16, u8> {
        &self.sapling_stages
    }

    // --- Door state -------------------------------------------------------------

    /// The door state (facing + open + which-half) at a local voxel, or `None` when no
    /// door is recorded there. See [`crate::door`] / `world::door`.
    #[inline]
    pub fn door_state(&self, x: usize, y: usize, z: usize) -> Option<DoorState> {
        self.doors.get(&Self::block_entity_key(x, y, z)).copied()
    }

    /// Record a door's `state` at a local voxel. Marks the chunk modified so the door
    /// persists. Does NOT change the block id — toggling open/closed leaves the door
    /// block in place (only its metadata moves), like a sapling's growth stage.
    pub fn set_door_state(&mut self, x: usize, y: usize, z: usize, state: DoorState) {
        self.doors
            .insert(Self::block_entity_key(x, y, z), state);
        self.modified = true;
    }

    /// Forget the door state at local index `i` — its cell is being overwritten
    /// (broken). Cheap no-op for the common chunk (empty map), mirroring
    /// [`clear_sapling_stage`](Self::clear_sapling_stage).
    #[inline]
    fn clear_door(&mut self, i: usize) {
        if !self.doors.is_empty() {
            self.doors.remove(&(i as u16));
        }
    }

    /// The door-state map, for saving (keyed by local block index).
    #[inline]
    pub fn doors(&self) -> &HashMap<u16, DoorState> {
        &self.doors
    }

    /// Water-flow metadata at a local voxel (0 where the cell is not flowing
    /// water or the column has never held flowing water). See `world::water`.
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

    /// Whether this column holds any random-tickable block — the gate the
    /// simulation uses to skip a whole column cheaply (see `world::tick`).
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
        self.mark_light_dirty();
    }

    /// Bulk water-flow metadata for saving (`None` if the column never held
    /// flowing water). Parallel to `blocks_slice`.
    pub fn water_slice(&self) -> Option<&[u8]> {
        self.water.as_deref()
    }

    // --- Furnace block-entities -------------------------------------------------

    /// Local block-index key for a block-entity map (`idx` fits a u16; see field
    /// doc). Shared by the furnace and chest maps — both are keyed by local index.
    #[inline]
    fn block_entity_key(x: usize, y: usize, z: usize) -> u16 {
        idx(x, y, z) as u16
    }

    #[inline]
    fn block_entity_coords(key: u16) -> (usize, usize, usize) {
        (
            (key & 0x000F) as usize,
            (key >> 8) as usize,
            ((key >> 4) & 0x000F) as usize,
        )
    }

    /// The furnace stored at a local voxel, if any.
    #[inline]
    pub fn furnace_at(&self, x: usize, y: usize, z: usize) -> Option<&Furnace> {
        self.furnaces.get(&Self::block_entity_key(x, y, z))
    }

    /// Mutable handle to the furnace at a local voxel (for GUI edits).
    #[inline]
    pub fn furnace_at_mut(&mut self, x: usize, y: usize, z: usize) -> Option<&mut Furnace> {
        self.furnaces.get_mut(&Self::block_entity_key(x, y, z))
    }

    /// Install `furnace` at a local voxel (block placement). Marks the chunk
    /// modified so the furnace persists from the moment it is placed.
    pub fn insert_furnace(&mut self, x: usize, y: usize, z: usize, furnace: Furnace) {
        self.furnaces
            .insert(Self::block_entity_key(x, y, z), furnace);
        self.modified = true;
    }

    /// Remove and return the furnace at a local voxel (block break), if any.
    pub fn take_furnace(&mut self, x: usize, y: usize, z: usize) -> Option<Furnace> {
        let removed = self.furnaces.remove(&Self::block_entity_key(x, y, z));
        if removed.is_some() {
            self.modified = true;
        }
        removed
    }

    /// Whether the furnace at a local voxel is currently lit — read by the mesher to
    /// pick the burning front texture. `false` when there is no furnace there.
    #[inline]
    pub fn is_furnace_lit(&self, x: usize, y: usize, z: usize) -> bool {
        self.furnace_at(x, y, z).is_some_and(Furnace::is_lit)
    }

    /// The facing of the furnace at a local voxel (which way its front points), or
    /// `North` if there is no furnace there. Read by the mesher to texture the front
    /// face vs the sides.
    #[inline]
    pub fn furnace_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.furnace_at(x, y, z)
            .map_or(Facing::default(), |f| f.facing)
    }

    /// The furnace map, for saving (parallel to `blocks_slice`).
    #[inline]
    pub fn furnaces(&self) -> &HashMap<u16, Furnace> {
        &self.furnaces
    }

    // --- Chest block-entities ---------------------------------------------------

    /// The chest stored at a local voxel, if any.
    #[inline]
    pub fn chest_at(&self, x: usize, y: usize, z: usize) -> Option<&Chest> {
        self.chests.get(&Self::block_entity_key(x, y, z))
    }

    /// Mutable handle to the chest at a local voxel (for GUI edits).
    #[inline]
    pub fn chest_at_mut(&mut self, x: usize, y: usize, z: usize) -> Option<&mut Chest> {
        self.chests.get_mut(&Self::block_entity_key(x, y, z))
    }

    /// Install `chest` at a local voxel (block placement). Marks the chunk modified
    /// so the chest persists from the moment it is placed.
    pub fn insert_chest(&mut self, x: usize, y: usize, z: usize, chest: Chest) {
        self.chests.insert(Self::block_entity_key(x, y, z), chest);
        self.modified = true;
    }

    /// Remove and return the chest at a local voxel (block break), if any.
    pub fn take_chest(&mut self, x: usize, y: usize, z: usize) -> Option<Chest> {
        let removed = self.chests.remove(&Self::block_entity_key(x, y, z));
        if removed.is_some() {
            self.modified = true;
        }
        removed
    }

    /// The chest map, for saving and for the renderer's per-frame gather (keyed by
    /// local block index — invert with `idx`: x = key & 15, z = (key >> 4) & 15,
    /// y = key >> 8).
    #[inline]
    pub fn chests(&self) -> &HashMap<u16, Chest> {
        &self.chests
    }

    // --- Torch orientation ------------------------------------------------------

    /// The placement of the torch at a local voxel, or `Floor` if none is recorded.
    /// Read by the mesher and the selection outline to orient the pole; the `Floor`
    /// default keeps a torch (somehow) missing its map entry rendering sanely rather
    /// than panicking.
    #[inline]
    pub fn torch_placement(&self, x: usize, y: usize, z: usize) -> TorchPlacement {
        self.torches
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    /// Record `placement` for the torch at a local voxel (block placement). Marks
    /// the chunk modified so the orientation persists from the moment it is placed.
    pub fn insert_torch(&mut self, x: usize, y: usize, z: usize, placement: TorchPlacement) {
        self.torches
            .insert(Self::block_entity_key(x, y, z), placement);
        self.modified = true;
    }

    /// Forget the torch orientation at a local voxel (block break). Marks the chunk
    /// modified when an entry was actually removed.
    pub fn take_torch(&mut self, x: usize, y: usize, z: usize) {
        if self
            .torches
            .remove(&Self::block_entity_key(x, y, z))
            .is_some()
        {
            self.modified = true;
        }
    }

    /// The torch-orientation map, for saving (keyed by local block index).
    #[inline]
    pub fn torches(&self) -> &HashMap<u16, TorchPlacement> {
        &self.torches
    }

    /// Advance every furnace in this chunk one game tick. `smelt(item)` yields an
    /// item's smelted product, supplied by the world layer from the recipe set so
    /// storage stays recipe-agnostic. Marks the chunk modified when any furnace
    /// state changed, and mesh-dirty when any furnace's lit state flipped (a texture
    /// change). Returns the local coordinates of furnaces whose lit texture changed
    /// so the world layer can enqueue the corresponding mesh and block updates.
    /// No-op for the common furnace-free chunk.
    pub fn tick_furnaces(
        &mut self,
        smelt: impl Fn(ItemType) -> Option<ItemStack>,
    ) -> Vec<(usize, usize, usize)> {
        if self.furnaces.is_empty() {
            return Vec::new();
        }
        let mut changed = false;
        let mut relit = Vec::new();
        for (&key, f) in self.furnaces.iter_mut() {
            let was_lit = f.is_lit();
            if f.tick(&smelt) {
                changed = true;
            }
            if f.is_lit() != was_lit {
                relit.push(Self::block_entity_coords(key));
            }
        }
        if changed {
            self.modified = true;
        }
        if !relit.is_empty() {
            self.dirty = true;
        }
        relit
    }

    /// Rebuild a chunk from saved arrays: block ids, biome ids, and optional
    /// water metadata. The heightmap is recomputed and skylight is left for the
    /// async bake. `modified` starts false — it already matches what's on disk.
    pub fn from_saved(
        cx: i32,
        cz: i32,
        blocks: Box<[u8]>,
        biomes_src: &[u8],
        water: Option<Box<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        chests: HashMap<u16, Chest>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        sapling_stages: HashMap<u16, u8>,
        doors: HashMap<u16, DoorState>,
    ) -> Self {
        let mut biomes = Box::new([0u8; CHUNK_SX * CHUNK_SZ]);
        biomes.copy_from_slice(biomes_src);
        let mut c = Self {
            cx,
            cz,
            blocks,
            random_tick_count: 0,
            water,
            furnaces,
            chests,
            torches,
            model_cells,
            model_facings,
            sapling_stages,
            doors,
            heightmap: Box::new([0u16; CHUNK_SX * CHUNK_SZ]),
            biomes,
            dirty: true,
            modified: false,
            skylight: Vec::new().into_boxed_slice(),
            sky_ylo: 0,
            sky_yhi: 0,
            blocklight: Vec::new().into_boxed_slice(),
            block_ylo: 0,
            block_yhi: 0,
            light_dirty: true,
            light_revision: 0,
        };
        c.recompute_heightmap();
        c.recompute_random_tick_count();
        c
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
