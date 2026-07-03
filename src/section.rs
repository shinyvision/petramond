//! Cubic section storage: a 16×16×16 voxel cube — the unit of the cubic-chunks
//! world. A vertical stack of sections sharing one `(cx,cz)` forms a column; the
//! inherently-2D per-column data (biome, surface heightmap, sky occlusion) lives
//! in [`crate::column::Column`], not here.
//!
//! This is the cubic successor to [`crate::chunk::Chunk`]: same battle-tested API
//! shape (block access, per-cell block-entity maps keyed by a `u16` local index,
//! water metadata, light, the random-tick gate) but scoped to one 16³ cube and
//! addressed by [`crate::chunk::section_idx`].

use std::collections::HashMap;
use std::sync::Arc;

use crate::block::{Block, BlockTag};
use crate::chest::Chest;
use crate::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::door::DoorState;
use crate::furnace::{Facing, Furnace};
use crate::item::{ItemStack, ItemType};
use crate::torch::TorchPlacement;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SectionSummary {
    Unknown,
    Empty,
    FullOpaque,
    FullWater,
    Mixed,
}

impl SectionSummary {
    #[inline]
    pub fn is_full_opaque(self) -> bool {
        matches!(self, SectionSummary::FullOpaque)
    }

    #[inline]
    pub fn virtual_block(self) -> Block {
        match self {
            SectionSummary::FullOpaque => Block::Stone,
            SectionSummary::FullWater => Block::Water,
            _ => Block::Air,
        }
    }
}

/// One 16³ cube of voxels. Blocks stored as a flat `Arc<[u8; 4096]>` indexed by
/// [`section_idx`].
///
/// `blocks` is an `Arc` so the off-thread light and mesh pools can take a cheap shared
/// reference to a section's block buffer (and its neighbours') without copying 4096 bytes
/// per section on the render thread — assembling the flood neighbourhood used to be a
/// multi-millisecond main-thread spike while streaming. Mutation is copy-on-write via
/// `Arc::make_mut`: a setter clones the buffer only if a bake is mid-flight against it.
///
/// `Clone` is derived so the mesher can take an owned snapshot of a section (plus its
/// neighbourhood) and build off the render thread.
#[derive(Clone)]
pub struct Section {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
    blocks: Arc<[u8]>,
    /// Per-block water state, parallel to `blocks`, only meaningful where the block
    /// is `Water`. Encodes flow `falloff` + a `FALLING` bit (see `world::water`).
    /// `None` until the section first holds non-source flowing water.
    ///
    /// `Arc` like [`blocks`](Self::blocks): when a section is shared with an in-flight bake,
    /// a copy-on-write edit ([`Arc::make_mut`] on the whole `Section`) must stay cheap, so
    /// every large buffer it holds is itself an `Arc` (clone = a refcount bump, not 4 KB).
    water: Option<Arc<[u8]>>,
    /// Furnace block-entities, keyed by section-local block index (`section_idx`,
    /// max 4095 — fits `u16`). Empty for the common section.
    furnaces: HashMap<u16, Furnace>,
    /// Chest block-entities, keyed like [`furnaces`](Self::furnaces).
    chests: HashMap<u16, Chest>,
    /// Torch orientations, keyed like [`furnaces`](Self::furnaces).
    torches: HashMap<u16, TorchPlacement>,
    /// Multi-cell bbmodel block occupancy (authored footprint offset for every cell
    /// whose offset is not `[0,0,0]`), keyed by section-local index.
    model_cells: HashMap<u16, [u8; 3]>,
    /// Per-cell facing for placed bbmodel blocks that need orientation.
    model_facings: HashMap<u16, Facing>,
    /// Growth stage (`0..=2`) of each sapling, keyed by section-local index. Absent
    /// reads as stage 0.
    sapling_stages: HashMap<u16, u8>,
    /// Door state (facing + open + which-half) of each door cell, keyed by
    /// section-local index.
    doors: HashMap<u16, DoorState>,
    /// Facing of each placed stair, keyed by section-local index. Absent stairs use
    /// the default north-facing shape, so old saves remain loadable.
    stair_facings: HashMap<u16, Facing>,
    pub dirty: bool,
    /// Set true by runtime edits, never by generation, so only player-touched
    /// sections are written to disk.
    pub modified: bool,
    /// Cached skylight (x2 scale), a full 16³ array indexed by [`section_idx`].
    /// `None` until first computed; an uncomputed section reads as open sky. `Arc` so the
    /// light drain can swap in a fresh cube on a shared section without a deep clone, and so
    /// a mesh job's snapshot keeps reading the old one safely.
    skylight: Option<Arc<[u8]>>,
    /// Cached block-light (x2 scale) radiated by emitters (torches). `None`/outside
    /// reads as 0 (no block light beyond the flood). `Arc` like [`skylight`](Self::skylight).
    blocklight: Option<Arc<[u8]>>,
    /// Set when blocks change; cleared when light is recomputed.
    pub light_dirty: bool,
    /// Bumped whenever cached light needs a new bake; async workers echo it back so
    /// stale results can be discarded.
    pub light_revision: u64,
    /// Bumped whenever this section is (re)queued for meshing; the async mesh worker
    /// echoes it back so a result built from a now-stale snapshot is discarded.
    pub mesh_revision: u64,
    /// Count of blocks in this section that receive random ticks. Maintained
    /// incrementally by every setter; the simulation skips the section when `0`.
    random_tick_count: u32,
    /// Count of OPAQUE cells. Maintained incrementally like `random_tick_count`. When it
    /// equals the section volume the section is fully solid: enclosed by other fully-solid
    /// sections it has no visible faces, so meshing, lighting, GPU upload, and per-frame
    /// drawing can all be skipped (the deep-stone fast path) until something carves air
    /// into it or a neighbour.
    opaque_count: u32,
    /// Opaque cells per 16×16 boundary plane, order [+X, −X, +Y, −Y, +Z, −Z].
    /// A count of 256 means that face of the section is fully walled: no sightline
    /// can cross it and every boundary face behind it is culled. Maintained by the
    /// setters and `recompute_opaque_count`; read by the sealed-section skip and
    /// the deep-section visibility BFS as O(1) plane-openness.
    plane_opaque: [u16; 6],
    /// Count of NON-AIR cells. `0` ⇒ the section is empty air, which emits no mesh faces
    /// at all (air draws nothing; solid neighbours draw their own faces toward it), so it
    /// is skipped from meshing/drawing unconditionally — the empty-sky fast path for the
    /// air band above the surface.
    non_air_count: u32,
    /// Count of Water cells. `0` ⇒ skip the streamed-water kick scan for this section
    /// (the vast majority of sections hold no water).
    water_count: u32,
    /// Count of cells whose emitted mesh can use biome tint. `0` lets meshing skip the
    /// biome halo/tint precompute for stone/cave/building sections.
    biome_tint_count: u32,
}

impl Section {
    pub fn new(cx: i32, cy: i32, cz: i32) -> Self {
        Self {
            cx,
            cy,
            cz,
            blocks: vec![0u8; SECTION_VOLUME].into(),
            water: None,
            furnaces: HashMap::new(),
            chests: HashMap::new(),
            torches: HashMap::new(),
            model_cells: HashMap::new(),
            model_facings: HashMap::new(),
            sapling_stages: HashMap::new(),
            doors: HashMap::new(),
            stair_facings: HashMap::new(),
            dirty: true,
            modified: false,
            skylight: None,
            blocklight: None,
            light_dirty: true,
            light_revision: 0,
            mesh_revision: 0,
            random_tick_count: 0,
            opaque_count: 0,
            plane_opaque: [0; 6],
            non_air_count: 0,
            water_count: 0,
            biome_tint_count: 0,
        }
    }

    /// World-space origin (minimum corner) of this section.
    #[inline]
    pub fn origin_world(&self) -> (i32, i32, i32) {
        (
            self.cx * SECTION_SIZE as i32,
            self.cy * SECTION_SIZE as i32,
            self.cz * SECTION_SIZE as i32,
        )
    }

    // --- Blocks -----------------------------------------------------------------

    #[inline]
    pub fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Block::from_id(self.blocks[section_idx(x, y, z)])
    }

    #[inline]
    pub fn block_raw(&self, x: usize, y: usize, z: usize) -> u8 {
        self.blocks[section_idx(x, y, z)]
    }

    pub fn set_block(&mut self, x: usize, y: usize, z: usize, b: Block) {
        self.set_block_raw(x, y, z, b.id());
    }

    pub fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        let i = section_idx(x, y, z);
        let blocks = Arc::make_mut(&mut self.blocks);
        let old = blocks[i];
        blocks[i] = id;
        self.adjust_random_tick_count(old, id);
        self.adjust_opaque_count(x, y, z, old, id);
        self.clear_water_meta(i);
        self.clear_model_cell(i);
        self.clear_sapling_stage(i);
        self.clear_door(i);
        self.clear_stair_facing(i);
        self.dirty = true;
        self.mark_light_dirty();
    }

    pub fn blocks_slice(&self) -> &[u8] {
        &self.blocks
    }
    pub fn blocks_slice_mut(&mut self) -> &mut [u8] {
        Arc::make_mut(&mut self.blocks)
    }

    /// A cheap shared handle to this section's block buffer (an `Arc` clone, no copy).
    /// The off-thread light/mesh pools take these for the flood/mesh neighbourhood instead
    /// of the render thread copying 4096 bytes per neighbour section.
    pub fn blocks_arc(&self) -> Arc<[u8]> {
        Arc::clone(&self.blocks)
    }

    /// Cheap shared handles to this section's water / skylight / block-light buffers (`Arc`
    /// clones, no copy; `None` when the buffer is absent). Used to snapshot a section's
    /// neighbour for off-thread meshing without deep-copying any voxel array.
    pub fn water_arc(&self) -> Option<Arc<[u8]>> {
        self.water.clone()
    }
    pub fn skylight_arc(&self) -> Option<Arc<[u8]>> {
        self.skylight.clone()
    }
    pub fn blocklight_arc(&self) -> Option<Arc<[u8]>> {
        self.blocklight.clone()
    }

    /// Whether a light bake has ever landed on this section (`set_skylight`).
    /// Distinguishes "stale light" (`light_dirty` after an edit) from "never lit",
    /// which the streamer defers differently.
    #[inline]
    pub fn has_baked_light(&self) -> bool {
        self.skylight.is_some()
    }

    // --- Water ------------------------------------------------------------------

    /// Water-flow metadata at a local voxel (0 where not flowing water).
    #[inline]
    pub fn water_meta(&self, x: usize, y: usize, z: usize) -> u8 {
        match &self.water {
            Some(w) => w[section_idx(x, y, z)],
            None => 0,
        }
    }

    /// Set a water cell (block + flow meta) WITHOUT marking skylight dirty: water
    /// is transparent, so flow updates only need a remesh. `meta` is treated as 0
    /// when `b` is not water.
    pub fn set_water(&mut self, x: usize, y: usize, z: usize, b: Block, meta: u8) {
        let i = section_idx(x, y, z);
        let id = b.id();
        let blocks = Arc::make_mut(&mut self.blocks);
        let old = blocks[i];
        blocks[i] = id;
        self.adjust_random_tick_count(old, id);
        self.adjust_opaque_count(x, y, z, old, id);
        let meta = if b == Block::Water { meta } else { 0 };
        self.store_water_meta(i, meta);
        self.dirty = true;
    }

    /// Bulk water-flow metadata for saving (`None` if never held flowing water).
    pub fn water_slice(&self) -> Option<&[u8]> {
        self.water.as_deref()
    }

    #[inline]
    fn clear_water_meta(&mut self, i: usize) {
        if let Some(w) = self.water.as_mut() {
            Arc::make_mut(w)[i] = 0;
        }
    }

    #[inline]
    fn store_water_meta(&mut self, i: usize, meta: u8) {
        if meta == 0 {
            self.clear_water_meta(i);
            return;
        }
        let w = self
            .water
            .get_or_insert_with(|| vec![0u8; SECTION_VOLUME].into());
        Arc::make_mut(w)[i] = meta;
    }

    // --- Light ------------------------------------------------------------------

    /// Skylight (x2 scale) at a local voxel. An uncomputed section reads as open
    /// sky (so a not-yet-lit section renders bright rather than black for the brief
    /// moment before its light bakes). Underground darkness is established once the
    /// band is computed (see `mesh::skylight`).
    #[inline]
    pub fn skylight_at(&self, x: usize, y: usize, z: usize) -> u8 {
        match &self.skylight {
            Some(s) => s[section_idx(x, y, z)],
            None => SKY_FULL,
        }
    }

    /// Install a freshly computed skylight cube and clear the dirty flag.
    pub fn set_skylight(&mut self, cube: Arc<[u8]>) {
        self.skylight = Some(cube);
        self.light_dirty = false;
    }

    /// Block-light (x2 scale) at a local voxel: `0` when uncomputed.
    #[inline]
    pub fn blocklight_at(&self, x: usize, y: usize, z: usize) -> u8 {
        match &self.blocklight {
            Some(b) => b[section_idx(x, y, z)],
            None => 0,
        }
    }

    /// Install a freshly computed block-light cube.
    pub fn set_blocklight(&mut self, cube: Arc<[u8]>) {
        self.blocklight = Some(cube);
    }

    pub fn mark_light_dirty(&mut self) {
        self.light_dirty = true;
        self.light_revision = self.light_revision.wrapping_add(1);
    }

    // --- Random-tick gate -------------------------------------------------------

    /// Keep [`random_tick_count`](Self::random_tick_count) in step with one cell
    /// changing from `old_id` to `new_id`.
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
    /// `blocks` directly instead of going through the setters.
    pub fn recompute_random_tick_count(&mut self) {
        self.random_tick_count = self
            .blocks
            .iter()
            .filter(|&&id| Block::from_id(id).has_random_tick())
            .count() as u32;
    }

    // --- Opaque (deep-stone) gate -----------------------------------------------

    /// Keep the opaque + non-air skip counters in step with one cell changing.
    #[inline]
    fn adjust_opaque_count(&mut self, x: usize, y: usize, z: usize, old_id: u8, new_id: u8) {
        let was_op = Block::from_id(old_id).is_opaque();
        let now_op = Block::from_id(new_id).is_opaque();
        match (was_op, now_op) {
            (false, true) => self.opaque_count += 1,
            (true, false) => self.opaque_count -= 1,
            _ => {}
        }
        if was_op != now_op {
            let d: i32 = if now_op { 1 } else { -1 };
            let hi = SECTION_SIZE - 1;
            let mut bump = |plane: usize| {
                self.plane_opaque[plane] = (self.plane_opaque[plane] as i32 + d) as u16;
            };
            if x == hi {
                bump(0);
            }
            if x == 0 {
                bump(1);
            }
            if y == hi {
                bump(2);
            }
            if y == 0 {
                bump(3);
            }
            if z == hi {
                bump(4);
            }
            if z == 0 {
                bump(5);
            }
        }
        let was_air = old_id == 0;
        let now_air = new_id == 0;
        match (was_air, now_air) {
            (true, false) => self.non_air_count += 1,
            (false, true) => self.non_air_count -= 1,
            _ => {}
        }
        let water_id = Block::Water.id();
        match (old_id == water_id, new_id == water_id) {
            (false, true) => self.water_count += 1,
            (true, false) => self.water_count -= 1,
            _ => {}
        }
        match (
            Self::id_uses_biome_tint(old_id),
            Self::id_uses_biome_tint(new_id),
        ) {
            (false, true) => self.biome_tint_count += 1,
            (true, false) => self.biome_tint_count -= 1,
            _ => {}
        }
    }

    /// Recount opaque + non-air + water + mesh hint cells — for a bulk load that fills
    /// `blocks` directly.
    pub fn recompute_opaque_count(&mut self) {
        let water_id = Block::Water.id();
        let mut opaque = 0u32;
        let mut non_air = 0u32;
        let mut water = 0u32;
        let mut biome_tint = 0u32;
        for &id in self.blocks.iter() {
            if Block::from_id(id).is_opaque() {
                opaque += 1;
            }
            if id != 0 {
                non_air += 1;
            }
            if id == water_id {
                water += 1;
            }
            if Self::id_uses_biome_tint(id) {
                biome_tint += 1;
            }
        }
        self.opaque_count = opaque;
        self.non_air_count = non_air;
        self.water_count = water;
        self.biome_tint_count = biome_tint;

        let mut planes = [0u16; 6];
        let hi = SECTION_SIZE - 1;
        for a in 0..SECTION_SIZE {
            for b in 0..SECTION_SIZE {
                let op = |x: usize, y: usize, z: usize| {
                    Block::from_id(self.blocks[section_idx(x, y, z)]).is_opaque() as u16
                };
                planes[0] += op(hi, a, b);
                planes[1] += op(0, a, b);
                planes[2] += op(a, hi, b);
                planes[3] += op(a, 0, b);
                planes[4] += op(a, b, hi);
                planes[5] += op(a, b, 0);
            }
        }
        self.plane_opaque = planes;
    }

    /// Whether every cell is opaque (fully solid). Such a section, when its six
    /// neighbours are also fully opaque, has no visible faces — meshing, lighting, and
    /// drawing it are pure waste, so the pipeline skips it.
    #[inline]
    pub fn all_opaque(&self) -> bool {
        self.opaque_count as usize == SECTION_VOLUME
    }

    /// Whether the section is entirely air. It emits no mesh faces, so it is skipped from
    /// meshing/drawing unconditionally (the empty-sky band above the surface).
    #[inline]
    pub fn is_empty_air(&self) -> bool {
        self.non_air_count == 0
    }

    /// Whether this section's 16×16 boundary plane facing `(dx,dy,dz)` (one unit axis
    /// step) is fully opaque. A fully-opaque plane admits no sightline across that face
    /// and culls every boundary face behind it, so six such planes seal the adjoining
    /// section — the buried-section mesh/light skip builds on this. O(1) from the
    /// per-plane counters.
    #[inline]
    pub fn face_plane_fully_opaque(&self, dx: i32, dy: i32, dz: i32) -> bool {
        const PLANE_AREA: u16 = (SECTION_SIZE * SECTION_SIZE) as u16;
        self.plane_opaque[Self::plane_index(dx, dy, dz)] == PLANE_AREA
    }

    /// Whether the boundary plane facing `(dx,dy,dz)` holds ANY non-opaque cell —
    /// i.e. a sightline (or an emitted boundary face) can exist on that face. The
    /// deep-section visibility BFS crosses section seams through open planes.
    #[inline]
    pub fn face_plane_open(&self, dx: i32, dy: i32, dz: i32) -> bool {
        !self.face_plane_fully_opaque(dx, dy, dz)
    }

    #[inline]
    fn plane_index(dx: i32, dy: i32, dz: i32) -> usize {
        debug_assert_eq!(dx.abs() + dy.abs() + dz.abs(), 1);
        match (dx, dy, dz) {
            (1, 0, 0) => 0,
            (-1, 0, 0) => 1,
            (0, 1, 0) => 2,
            (0, -1, 0) => 3,
            (0, 0, 1) => 4,
            _ => 5,
        }
    }

    /// Whether the section holds any Water cell. The streamed-water kick scans only these.
    #[inline]
    pub fn has_water(&self) -> bool {
        self.water_count > 0
    }

    /// Whether this section can emit any biome-tinted mesh face.
    #[inline]
    pub fn has_biome_tint_blocks(&self) -> bool {
        self.biome_tint_count > 0
    }

    /// Whether the section holds any air cell.
    #[inline]
    pub fn has_air(&self) -> bool {
        (self.non_air_count as usize) < SECTION_VOLUME
    }

    #[inline]
    pub fn summary(&self) -> SectionSummary {
        if self.is_empty_air() {
            SectionSummary::Empty
        } else if self.all_opaque() {
            SectionSummary::FullOpaque
        } else if self.water_count as usize == SECTION_VOLUME {
            SectionSummary::FullWater
        } else {
            SectionSummary::Mixed
        }
    }

    /// Whether this section holds any random-tickable block — the gate the
    /// simulation uses to skip a section cheaply.
    #[inline]
    pub fn has_random_tickable(&self) -> bool {
        self.random_tick_count > 0
    }

    #[inline]
    fn id_uses_biome_tint(id: u8) -> bool {
        let block = Block::from_id(id);
        matches!(
            block,
            Block::Grass | Block::Water | Block::ShortGrass | Block::Fern
        ) || block.has_tag(BlockTag::Leaves)
    }

    #[cfg(all(test, feature = "worldgen-tests"))]
    pub(crate) fn random_tick_count(&self) -> u32 {
        self.random_tick_count
    }

    // --- Block-entity maps ------------------------------------------------------

    /// Section-local block-index key for a block-entity map (`section_idx` fits a
    /// `u16`).
    #[inline]
    fn block_entity_key(x: usize, y: usize, z: usize) -> u16 {
        section_idx(x, y, z) as u16
    }

    /// Invert [`block_entity_key`](Self::block_entity_key): `x = key & 15`,
    /// `y = key >> 8`, `z = (key >> 4) & 15`.
    #[inline]
    fn block_entity_coords(key: u16) -> (usize, usize, usize) {
        (
            (key & 0x000F) as usize,
            (key >> 8) as usize,
            ((key >> 4) & 0x000F) as usize,
        )
    }

    #[inline]
    fn clear_model_cell(&mut self, i: usize) {
        if !self.model_cells.is_empty() {
            self.model_cells.remove(&(i as u16));
        }
        if !self.model_facings.is_empty() {
            self.model_facings.remove(&(i as u16));
        }
    }

    #[inline]
    pub fn set_model_offset(&mut self, x: usize, y: usize, z: usize, offset: [u8; 3]) {
        self.model_cells
            .insert(Self::block_entity_key(x, y, z), offset);
        self.dirty = true;
    }

    #[inline]
    pub fn model_offset(&self, x: usize, y: usize, z: usize) -> [u8; 3] {
        self.model_cells
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or([0, 0, 0])
    }

    #[inline]
    pub fn set_model_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.model_facings
            .insert(Self::block_entity_key(x, y, z), facing);
        self.dirty = true;
    }

    #[inline]
    pub fn model_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.model_facings
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or(crate::block_model::DEFAULT_MODEL_FACING)
    }

    #[inline]
    pub fn model_cells(&self) -> &HashMap<u16, [u8; 3]> {
        &self.model_cells
    }

    #[inline]
    pub fn model_facings(&self) -> &HashMap<u16, Facing> {
        &self.model_facings
    }

    #[inline]
    pub fn sapling_stage(&self, x: usize, y: usize, z: usize) -> u8 {
        self.sapling_stages
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or(0)
    }

    pub fn set_sapling_stage(&mut self, x: usize, y: usize, z: usize, stage: u8) {
        let key = Self::block_entity_key(x, y, z);
        if stage == 0 {
            self.sapling_stages.remove(&key);
        } else {
            self.sapling_stages.insert(key, stage);
        }
        self.modified = true;
    }

    #[inline]
    fn clear_sapling_stage(&mut self, i: usize) {
        if !self.sapling_stages.is_empty() {
            self.sapling_stages.remove(&(i as u16));
        }
    }

    #[inline]
    pub fn sapling_stages(&self) -> &HashMap<u16, u8> {
        &self.sapling_stages
    }

    #[inline]
    pub fn door_state(&self, x: usize, y: usize, z: usize) -> Option<DoorState> {
        self.doors.get(&Self::block_entity_key(x, y, z)).copied()
    }

    pub fn set_door_state(&mut self, x: usize, y: usize, z: usize, state: DoorState) {
        self.doors.insert(Self::block_entity_key(x, y, z), state);
        self.modified = true;
    }

    #[inline]
    fn clear_door(&mut self, i: usize) {
        if !self.doors.is_empty() {
            self.doors.remove(&(i as u16));
        }
    }

    #[inline]
    pub fn doors(&self) -> &HashMap<u16, DoorState> {
        &self.doors
    }

    #[inline]
    pub fn stair_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.stair_facings
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    pub fn set_stair_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.stair_facings
            .insert(Self::block_entity_key(x, y, z), facing);
        self.modified = true;
    }

    #[inline]
    fn clear_stair_facing(&mut self, i: usize) {
        if !self.stair_facings.is_empty() {
            self.stair_facings.remove(&(i as u16));
        }
    }

    #[inline]
    pub fn stair_facings(&self) -> &HashMap<u16, Facing> {
        &self.stair_facings
    }

    #[inline]
    pub fn furnace_at(&self, x: usize, y: usize, z: usize) -> Option<&Furnace> {
        self.furnaces.get(&Self::block_entity_key(x, y, z))
    }

    #[inline]
    pub fn furnace_at_mut(&mut self, x: usize, y: usize, z: usize) -> Option<&mut Furnace> {
        self.furnaces.get_mut(&Self::block_entity_key(x, y, z))
    }

    pub fn insert_furnace(&mut self, x: usize, y: usize, z: usize, furnace: Furnace) {
        self.furnaces
            .insert(Self::block_entity_key(x, y, z), furnace);
        self.modified = true;
    }

    pub fn take_furnace(&mut self, x: usize, y: usize, z: usize) -> Option<Furnace> {
        let removed = self.furnaces.remove(&Self::block_entity_key(x, y, z));
        if removed.is_some() {
            self.modified = true;
        }
        removed
    }

    #[inline]
    pub fn is_furnace_lit(&self, x: usize, y: usize, z: usize) -> bool {
        self.furnace_at(x, y, z).is_some_and(Furnace::is_lit)
    }

    #[inline]
    pub fn furnace_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.furnace_at(x, y, z)
            .map_or(Facing::default(), |f| f.facing)
    }

    #[inline]
    pub fn furnaces(&self) -> &HashMap<u16, Furnace> {
        &self.furnaces
    }

    #[inline]
    pub fn chest_at(&self, x: usize, y: usize, z: usize) -> Option<&Chest> {
        self.chests.get(&Self::block_entity_key(x, y, z))
    }

    #[inline]
    pub fn chest_at_mut(&mut self, x: usize, y: usize, z: usize) -> Option<&mut Chest> {
        self.chests.get_mut(&Self::block_entity_key(x, y, z))
    }

    pub fn insert_chest(&mut self, x: usize, y: usize, z: usize, chest: Chest) {
        self.chests.insert(Self::block_entity_key(x, y, z), chest);
        self.modified = true;
    }

    pub fn take_chest(&mut self, x: usize, y: usize, z: usize) -> Option<Chest> {
        let removed = self.chests.remove(&Self::block_entity_key(x, y, z));
        if removed.is_some() {
            self.modified = true;
        }
        removed
    }

    #[inline]
    pub fn chests(&self) -> &HashMap<u16, Chest> {
        &self.chests
    }

    #[inline]
    pub fn torch_placement(&self, x: usize, y: usize, z: usize) -> TorchPlacement {
        self.torches
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    pub fn insert_torch(&mut self, x: usize, y: usize, z: usize, placement: TorchPlacement) {
        self.torches
            .insert(Self::block_entity_key(x, y, z), placement);
        self.modified = true;
    }

    pub fn take_torch(&mut self, x: usize, y: usize, z: usize) {
        if self
            .torches
            .remove(&Self::block_entity_key(x, y, z))
            .is_some()
        {
            self.modified = true;
        }
    }

    #[inline]
    pub fn torches(&self) -> &HashMap<u16, TorchPlacement> {
        &self.torches
    }

    /// Advance every furnace one game tick. Returns the local coordinates of
    /// furnaces whose lit texture changed (so the world can enqueue mesh/block
    /// updates). No-op for the common furnace-free section.
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

    /// Rebuild a section from saved arrays. `modified` starts false — it already
    /// matches what's on disk. The light cube is left for the async bake.
    #[allow(clippy::too_many_arguments)]
    pub fn from_saved(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: Box<[u8]>,
        water: Option<Box<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        chests: HashMap<u16, Chest>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        sapling_stages: HashMap<u16, u8>,
        doors: HashMap<u16, DoorState>,
        stair_facings: HashMap<u16, Facing>,
    ) -> Self {
        let mut s = Self {
            cx,
            cy,
            cz,
            blocks: blocks.into(),
            water: water.map(Arc::from),
            furnaces,
            chests,
            torches,
            model_cells,
            model_facings,
            sapling_stages,
            doors,
            stair_facings,
            dirty: true,
            modified: false,
            skylight: None,
            blocklight: None,
            light_dirty: true,
            light_revision: 0,
            mesh_revision: 0,
            random_tick_count: 0,
            opaque_count: 0,
            plane_opaque: [0; 6],
            non_air_count: 0,
            water_count: 0,
            biome_tint_count: 0,
        };
        s.recompute_random_tick_count();
        s.recompute_opaque_count();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biome_tint_hint_tracks_incremental_and_bulk_blocks() {
        let mut section = Section::new(0, 0, 0);
        assert!(!section.has_biome_tint_blocks());

        section.set_block(1, 1, 1, Block::Stone);
        assert!(!section.has_biome_tint_blocks());

        section.set_block(1, 1, 1, Block::Grass);
        assert!(section.has_biome_tint_blocks());

        section.set_block(1, 1, 1, Block::Dirt);
        assert!(!section.has_biome_tint_blocks());

        section.set_water(2, 2, 2, Block::Water, 0);
        assert!(section.has_biome_tint_blocks());

        section.set_block(2, 2, 2, Block::Air);
        assert!(!section.has_biome_tint_blocks());

        section.blocks_slice_mut()[0] = Block::OakLeaves.id();
        section.recompute_opaque_count();
        assert!(section.has_biome_tint_blocks());
    }
}
