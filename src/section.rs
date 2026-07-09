//! Cubic section storage: a 16×16×16 voxel cube — the unit of the cubic-chunks
//! world. A vertical stack of sections sharing one `(cx,cz)` forms a column; the
//! inherently-2D per-column data (biome, surface heightmap, sky occlusion) lives
//! in [`crate::column::Column`], not here.
//!
//! This is the cubic successor to [`crate::chunk::Chunk`]: same battle-tested API
//! shape (block access, per-cell block-entity maps keyed by a `u16` local index,
//! water metadata, light, the random-tick gate) but scoped to one 16³ cube and
//! addressed by [`crate::chunk::section_idx`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::block::{Block, BlockTag};
use crate::block_state::{BlockStates, LogAxis, SlabState, StairHalf, StairState};
use crate::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::container::Container;
use crate::door::DoorState;
use crate::facing::Facing;
use crate::furnace::Furnace;
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
    pub fn virtual_block(self) -> Block {
        match self {
            SectionSummary::FullOpaque => Block::Stone,
            SectionSummary::FullWater => Block::Water,
            _ => Block::Air,
        }
    }

    /// Stable byte for the wire (`ColumnPayload::summaries`).
    #[inline]
    pub fn to_u8(self) -> u8 {
        match self {
            SectionSummary::Unknown => 0,
            SectionSummary::Empty => 1,
            SectionSummary::FullOpaque => 2,
            SectionSummary::FullWater => 3,
            SectionSummary::Mixed => 4,
        }
    }

    /// Inverse of [`to_u8`](Self::to_u8); unknown bytes read as `Unknown`
    /// (the conservative "reads lie" answer).
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => SectionSummary::Empty,
            2 => SectionSummary::FullOpaque,
            3 => SectionSummary::FullWater,
            4 => SectionSummary::Mixed,
            _ => SectionSummary::Unknown,
        }
    }
}

/// Counter and boundary-plane metadata that can travel with an immutable block
/// buffer. Loopback replicas adopt it directly instead of rescanning 4,096 cells;
/// network remapping recomputes it on the transport thread when ids change.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SectionMetrics {
    pub random_tick_count: u32,
    pub opaque_count: u32,
    pub plane_opaque: [u16; 6],
    pub non_air_count: u32,
    pub water_count: u32,
    pub biome_tint_count: u32,
    pub particle_emitter_count: u32,
}

impl SectionMetrics {
    pub(crate) fn valid(self) -> bool {
        let volume = SECTION_VOLUME as u32;
        self.random_tick_count <= volume
            && self.opaque_count <= volume
            && self.non_air_count <= volume
            && self.water_count <= volume
            && self.biome_tint_count <= volume
            && self.particle_emitter_count <= volume
            && self.plane_opaque.iter().all(|&n| n <= 256)
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
/// Block ids stay dense and minimal. Per-cell state that changes block behavior or
/// rendering lives in `states`, which keeps uncommon states sparse and centralized.
#[derive(Clone)]
pub struct Section {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
    blocks: Arc<[u8]>,
    states: BlockStates,
    /// Block-entity state, allocated on first insert — `None` for the common
    /// generated section (together with `BlockStates`' boxed sparse maps this
    /// keeps `size_of::<Section>()` small across thousands of loaded sections).
    entities: Option<Box<BlockEntities>>,
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
    /// equals the section volume the section is fully solid: its cells carry no light, so
    /// neighbours' mesh jobs skip waiting on (and requesting) its light bake.
    opaque_count: u32,
    /// Opaque cells per 16×16 boundary plane, order [+X, −X, +Y, −Y, +Z, −Z].
    /// A count of 256 means that face of the section is fully walled: no sightline
    /// can cross it and every boundary face behind it is culled. Maintained by the
    /// setters and `recompute_opaque_count`; read by the deep-section visibility
    /// BFS as O(1) plane-openness.
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
    /// Count of cells whose block row declares a visual particle emitter. `0` lets
    /// presentation skip this section when collecting ambient block emitters.
    particle_emitter_count: u32,
}

/// A section's block-entity maps, keyed by section-local block index
/// (`section_idx`, max 4095 — fits `u16`).
#[derive(Clone, Default)]
struct BlockEntities {
    /// Furnace machine state (burn/cook counters). A furnace's SLOTS live in
    /// [`containers`](Self::containers) under the same key.
    furnaces: HashMap<u16, Furnace>,
    /// Generic item-slot containers — chests, furnaces, and mod container
    /// blocks all store their stacks here.
    containers: HashMap<u16, Container>,
    /// Which way a facing block-entity (chest, furnace) points.
    entity_facings: HashMap<u16, Facing>,
}

impl BlockEntities {
    fn is_empty(&self) -> bool {
        self.furnaces.is_empty() && self.containers.is_empty() && self.entity_facings.is_empty()
    }
}

/// Process-wide shared 16³ cubes filled with one byte value. Uniform sections
/// (all-air sky band, all-stone deep, all-water ocean) and uniform light cubes
/// (open sky, pitch dark) point at these instead of owning a 4 KiB buffer each;
/// the cache entry keeps the refcount ≥ 2, so the first heterogeneous write
/// un-shares through the existing `Arc::make_mut` copy-on-write path.
fn uniform_cube(value: u8) -> Arc<[u8]> {
    static CACHE: [std::sync::OnceLock<Arc<[u8]>>; 256] =
        [const { std::sync::OnceLock::new() }; 256];
    CACHE[value as usize]
        .get_or_init(|| vec![value; SECTION_VOLUME].into())
        .clone()
}

/// Collapse `cube` onto the shared per-value buffer when all its cells are equal.
fn compact_uniform_cube(cube: Arc<[u8]>) -> Arc<[u8]> {
    let first = cube[0];
    if cube.iter().all(|&v| v == first) {
        uniform_cube(first)
    } else {
        cube
    }
}

impl Section {
    pub fn new(cx: i32, cy: i32, cz: i32) -> Self {
        Self {
            cx,
            cy,
            cz,
            blocks: uniform_cube(0),
            states: BlockStates::new(),
            entities: None,
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
            particle_emitter_count: 0,
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
        self.states.clear_on_block_change(i);
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
        self.states.water_arc()
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
        self.states.water_meta(section_idx(x, y, z))
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
        self.states.store_water_meta(i, meta);
        self.dirty = true;
    }

    /// Bulk water-flow metadata for saving (`None` if never held flowing water).
    pub fn water_slice(&self) -> Option<&[u8]> {
        self.states.water_slice()
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
    /// Uniform cubes (fully open sky above the surface, fully dark deep
    /// underground — most lit sections) collapse onto the shared per-value
    /// buffer instead of retaining the bake's 4 KiB allocation.
    pub fn set_skylight(&mut self, cube: Arc<[u8]>) {
        self.skylight = Some(compact_uniform_cube(cube));
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

    /// Install a freshly computed block-light cube. All-zero (no emitter in
    /// range — the overwhelmingly common bake) stores as `None`, which reads
    /// identically; other uniform cubes share the per-value buffer.
    pub fn set_blocklight(&mut self, cube: Arc<[u8]>) {
        if cube.iter().all(|&v| v == 0) {
            self.blocklight = None;
        } else {
            self.blocklight = Some(compact_uniform_cube(cube));
        }
    }

    pub fn mark_light_dirty(&mut self) {
        self.light_dirty = true;
        self.light_revision = self.light_revision.wrapping_add(1);
    }

    /// Mark light final as-is WITHOUT installing cubes. Replica-only: light is
    /// server-owned there — a lightless install is a fully-opaque section
    /// (final by definition), and an edited cell keeps sampling the old cubes
    /// until the server's rebake arrives as `LightData`. Nothing on a replica
    /// may ever request a local bake.
    pub fn mark_light_clean(&mut self) {
        self.light_dirty = false;
    }

    /// Drop the block-light cubes (a rebake that reaches no emitter ships no
    /// buffer; absent reads as all-zero — see [`Self::set_blocklight`]).
    pub fn clear_blocklight(&mut self) {
        self.blocklight = None;
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
        match (
            Self::id_has_particle_emitter(old_id),
            Self::id_has_particle_emitter(new_id),
        ) {
            (false, true) => self.particle_emitter_count += 1,
            (true, false) => self.particle_emitter_count -= 1,
            _ => {}
        }
    }

    /// Compute every block-derived counter in one pass, including boundary planes.
    pub(crate) fn metrics_from_blocks(blocks: &[u8]) -> SectionMetrics {
        if blocks.len() != SECTION_VOLUME {
            return SectionMetrics::default();
        }
        let water_id = Block::Water.id();
        let mut out = SectionMetrics::default();
        let hi = SECTION_SIZE - 1;
        for (idx, &id) in blocks.iter().enumerate() {
            let block = Block::from_id(id);
            if block.has_random_tick() {
                out.random_tick_count += 1;
            }
            if block.is_opaque() {
                out.opaque_count += 1;
                let x = idx % SECTION_SIZE;
                let y = idx / (SECTION_SIZE * SECTION_SIZE);
                let z = (idx / SECTION_SIZE) % SECTION_SIZE;
                if x == hi {
                    out.plane_opaque[0] += 1;
                }
                if x == 0 {
                    out.plane_opaque[1] += 1;
                }
                if y == hi {
                    out.plane_opaque[2] += 1;
                }
                if y == 0 {
                    out.plane_opaque[3] += 1;
                }
                if z == hi {
                    out.plane_opaque[4] += 1;
                }
                if z == 0 {
                    out.plane_opaque[5] += 1;
                }
            }
            if id != 0 {
                out.non_air_count += 1;
            }
            if id == water_id {
                out.water_count += 1;
            }
            if Self::id_uses_biome_tint(id) {
                out.biome_tint_count += 1;
            }
            if Self::id_has_particle_emitter(id) {
                out.particle_emitter_count += 1;
            }
        }
        out
    }

    fn install_metrics(&mut self, metrics: SectionMetrics) {
        self.random_tick_count = metrics.random_tick_count;
        self.opaque_count = metrics.opaque_count;
        self.plane_opaque = metrics.plane_opaque;
        self.non_air_count = metrics.non_air_count;
        self.water_count = metrics.water_count;
        self.biome_tint_count = metrics.biome_tint_count;
        self.particle_emitter_count = metrics.particle_emitter_count;
    }

    pub(crate) fn stream_metrics(&self) -> SectionMetrics {
        SectionMetrics {
            random_tick_count: self.random_tick_count,
            opaque_count: self.opaque_count,
            plane_opaque: self.plane_opaque,
            non_air_count: self.non_air_count,
            water_count: self.water_count,
            biome_tint_count: self.biome_tint_count,
            particle_emitter_count: self.particle_emitter_count,
        }
    }

    /// Recount opaque + non-air + water + mesh/presentation hint cells — for a bulk
    /// load that fills `blocks` directly.
    pub fn recompute_opaque_count(&mut self) {
        self.install_metrics(Self::metrics_from_blocks(&self.blocks));
        self.compact_uniform_blocks();
    }

    /// Swap the block buffer for the shared per-id uniform cube when every cell
    /// holds the same id (all-air, all-stone, all-water — the bulk of loaded
    /// sections). Runs from `recompute_opaque_count`, so every bulk-load path
    /// compacts automatically. Counter fast paths gate the byte scan to sections
    /// that can actually be uniform.
    fn compact_uniform_blocks(&mut self) {
        let uniform_id = if self.non_air_count == 0 {
            Some(0u8)
        } else if self.opaque_count as usize == SECTION_VOLUME
            || self.water_count as usize == SECTION_VOLUME
        {
            let first = self.blocks[0];
            self.blocks.iter().all(|&b| b == first).then_some(first)
        } else {
            None
        };
        if let Some(id) = uniform_id {
            self.blocks = uniform_cube(id);
        }
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
    /// and culls every boundary face behind it; the deep-section visibility BFS treats
    /// such planes as closed. O(1) from the per-plane counters.
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

    /// Whether this section contains any block-row particle emitter.
    #[inline]
    pub fn has_particle_emitters(&self) -> bool {
        self.particle_emitter_count > 0
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
        ) || block.has_tag(BlockTag::LEAVES)
    }

    #[inline]
    fn id_has_particle_emitter(id: u8) -> bool {
        Block::from_id(id).particle_emitter().is_some()
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
    pub fn set_model_offset(&mut self, x: usize, y: usize, z: usize, offset: [u8; 3]) {
        self.states.set_model_offset(x, y, z, offset);
        self.dirty = true;
    }

    #[inline]
    pub fn model_offset(&self, x: usize, y: usize, z: usize) -> [u8; 3] {
        self.states.model_offset(x, y, z)
    }

    #[inline]
    pub fn set_model_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.states.set_model_facing(x, y, z, facing);
        self.dirty = true;
    }

    #[inline]
    pub fn model_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.states.model_facing(x, y, z)
    }

    #[inline]
    pub fn model_cells(&self) -> &HashMap<u16, [u8; 3]> {
        self.states.model_cells()
    }

    #[inline]
    pub fn model_facings(&self) -> &HashMap<u16, Facing> {
        self.states.model_facings()
    }

    #[inline]
    pub fn sapling_stage(&self, x: usize, y: usize, z: usize) -> u8 {
        self.states.sapling_stage(x, y, z)
    }

    pub fn set_sapling_stage(&mut self, x: usize, y: usize, z: usize, stage: u8) {
        self.states.set_sapling_stage(x, y, z, stage);
        self.modified = true;
    }

    #[inline]
    pub fn sapling_stages(&self) -> &HashMap<u16, u8> {
        self.states.sapling_stages()
    }

    #[inline]
    pub fn door_state(&self, x: usize, y: usize, z: usize) -> Option<DoorState> {
        self.states.door_state(x, y, z)
    }

    pub fn set_door_state(&mut self, x: usize, y: usize, z: usize, state: DoorState) {
        self.states.set_door_state(x, y, z, state);
        self.modified = true;
    }

    #[inline]
    pub fn doors(&self) -> &HashMap<u16, DoorState> {
        self.states.doors()
    }

    #[inline]
    pub fn stair_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.stair_state(x, y, z).facing
    }

    pub fn set_stair_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.set_stair_state(x, y, z, StairState::new(facing, StairHalf::Bottom));
    }

    #[inline]
    pub fn stair_state(&self, x: usize, y: usize, z: usize) -> StairState {
        self.states.stair_state(x, y, z)
    }

    pub fn set_stair_state(&mut self, x: usize, y: usize, z: usize, state: StairState) {
        self.states.set_stair_state(x, y, z, state);
        self.modified = true;
    }

    #[inline]
    pub fn log_axis(&self, x: usize, y: usize, z: usize) -> LogAxis {
        self.states.log_axis(x, y, z)
    }

    pub fn set_log_axis(&mut self, x: usize, y: usize, z: usize, axis: LogAxis) {
        self.states.set_log_axis(x, y, z, axis);
        self.modified = true;
    }

    #[inline]
    pub fn log_axes(&self) -> &HashMap<u16, LogAxis> {
        self.states.log_axes()
    }

    #[inline]
    /// A cell's mod KV entry, or `None` when the cell (or key) has none.
    pub fn cell_kv_get(&self, x: usize, y: usize, z: usize, key: &str) -> Option<&[u8]> {
        self.states.cell_kv_get(x, y, z, key)
    }

    /// Store a cell mod KV entry. Does NOT set `modified` — the world-level
    /// wrapper owns that (mirroring the block-entity insert pattern).
    pub fn cell_kv_set(&mut self, x: usize, y: usize, z: usize, key: String, value: Vec<u8>) {
        self.states.cell_kv_set(x, y, z, key, value);
    }

    /// Remove a cell mod KV entry; returns whether it was present. An inner
    /// map emptied by the removal is dropped whole, so the save codec's
    /// has-cell-kv flag clears once the last entry goes (the stale-record
    /// guard pattern — see WIKI/save-entities.md).
    pub fn cell_kv_remove(&mut self, x: usize, y: usize, z: usize, key: &str) -> bool {
        self.states.cell_kv_remove(x, y, z, key)
    }

    /// The whole per-cell mod KV map, for the save codec.
    pub fn cell_kv(&self) -> &HashMap<u16, BTreeMap<String, Vec<u8>>> {
        self.states.cell_kv()
    }

    /// Detach one cell's whole mod-KV map — the state-preserving half of a
    /// model-block swap (see `World::swap_model_block`).
    pub fn cell_kv_take(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
    ) -> Option<BTreeMap<String, Vec<u8>>> {
        self.states.cell_kv_take(x, y, z)
    }

    /// Re-attach a map detached by [`cell_kv_take`](Self::cell_kv_take).
    pub fn cell_kv_restore(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        map: BTreeMap<String, Vec<u8>>,
    ) {
        self.states.cell_kv_restore(x, y, z, map);
    }

    pub fn stair_states(&self) -> &HashMap<u16, StairState> {
        self.states.stair_states()
    }

    #[inline]
    pub fn slab_state(&self, x: usize, y: usize, z: usize) -> SlabState {
        self.states.slab_state(x, y, z)
    }

    pub fn set_slab_state(&mut self, x: usize, y: usize, z: usize, state: SlabState) {
        self.states.set_slab_state(x, y, z, state);
        self.modified = true;
    }

    pub fn slab_states(&self) -> &HashMap<u16, SlabState> {
        self.states.slab_states()
    }

    #[inline]
    fn entities_mut(&mut self) -> &mut BlockEntities {
        self.entities.get_or_insert_default()
    }

    #[inline]
    pub fn furnace_at(&self, x: usize, y: usize, z: usize) -> Option<&Furnace> {
        self.entities
            .as_deref()
            .and_then(|e| e.furnaces.get(&Self::block_entity_key(x, y, z)))
    }

    #[inline]
    pub fn furnace_at_mut(&mut self, x: usize, y: usize, z: usize) -> Option<&mut Furnace> {
        self.entities
            .as_deref_mut()
            .and_then(|e| e.furnaces.get_mut(&Self::block_entity_key(x, y, z)))
    }

    pub fn insert_furnace(&mut self, x: usize, y: usize, z: usize, furnace: Furnace) {
        self.entities_mut()
            .furnaces
            .insert(Self::block_entity_key(x, y, z), furnace);
        self.modified = true;
    }

    pub fn take_furnace(&mut self, x: usize, y: usize, z: usize) -> Option<Furnace> {
        let removed = self
            .entities
            .as_deref_mut()
            .and_then(|e| e.furnaces.remove(&Self::block_entity_key(x, y, z)));
        if removed.is_some() {
            self.modified = true;
        }
        removed
    }

    /// The furnace state and its container slots at one cell, split-borrowed
    /// (they live in sibling maps under the same key).
    pub fn furnace_parts_mut(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
    ) -> Option<(&mut Furnace, &mut Container)> {
        let key = Self::block_entity_key(x, y, z);
        let e = self.entities.as_deref_mut()?;
        let furnace = e.furnaces.get_mut(&key)?;
        let container = e.containers.get_mut(&key)?;
        Some((furnace, container))
    }

    #[inline]
    pub fn is_furnace_lit(&self, x: usize, y: usize, z: usize) -> bool {
        self.furnace_at(x, y, z).is_some_and(Furnace::is_lit)
    }

    #[inline]
    pub fn furnaces(&self) -> &HashMap<u16, Furnace> {
        match &self.entities {
            Some(e) => &e.furnaces,
            None => crate::block_state::empty_map!(Furnace),
        }
    }

    /// Which way the facing block-entity (chest, furnace) at a cell points.
    #[inline]
    pub fn entity_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.entity_facings()
            .get(&Self::block_entity_key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    pub fn insert_entity_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.entities_mut()
            .entity_facings
            .insert(Self::block_entity_key(x, y, z), facing);
        self.modified = true;
    }

    pub fn take_entity_facing(&mut self, x: usize, y: usize, z: usize) {
        if self
            .entities
            .as_deref_mut()
            .and_then(|e| e.entity_facings.remove(&Self::block_entity_key(x, y, z)))
            .is_some()
        {
            self.modified = true;
        }
    }

    #[inline]
    pub fn entity_facings(&self) -> &HashMap<u16, Facing> {
        match &self.entities {
            Some(e) => &e.entity_facings,
            None => crate::block_state::empty_map!(Facing),
        }
    }

    #[inline]
    pub fn container_at(&self, x: usize, y: usize, z: usize) -> Option<&Container> {
        self.entities
            .as_deref()
            .and_then(|e| e.containers.get(&Self::block_entity_key(x, y, z)))
    }

    #[inline]
    pub fn container_at_mut(&mut self, x: usize, y: usize, z: usize) -> Option<&mut Container> {
        self.entities
            .as_deref_mut()
            .and_then(|e| e.containers.get_mut(&Self::block_entity_key(x, y, z)))
    }

    pub fn insert_container(&mut self, x: usize, y: usize, z: usize, c: Container) {
        self.entities_mut()
            .containers
            .insert(Self::block_entity_key(x, y, z), c);
        self.modified = true;
    }

    pub fn take_container(&mut self, x: usize, y: usize, z: usize) -> Option<Container> {
        let removed = self
            .entities
            .as_deref_mut()
            .and_then(|e| e.containers.remove(&Self::block_entity_key(x, y, z)));
        if removed.is_some() {
            self.modified = true;
        }
        removed
    }

    #[inline]
    pub fn containers(&self) -> &HashMap<u16, Container> {
        match &self.entities {
            Some(e) => &e.containers,
            None => crate::block_state::empty_map!(Container),
        }
    }

    #[inline]
    pub fn torch_placement(&self, x: usize, y: usize, z: usize) -> TorchPlacement {
        self.states.torch_placement(x, y, z)
    }

    pub fn insert_torch(&mut self, x: usize, y: usize, z: usize, placement: TorchPlacement) {
        self.states.insert_torch(x, y, z, placement);
        self.modified = true;
    }

    pub fn take_torch(&mut self, x: usize, y: usize, z: usize) {
        if self.states.take_torch(x, y, z) {
            self.modified = true;
        }
    }

    #[inline]
    pub fn torches(&self) -> &HashMap<u16, TorchPlacement> {
        self.states.torches()
    }

    /// Advance every furnace one game tick. Returns the local coordinates of
    /// furnaces whose lit texture changed (so the world can enqueue mesh/block
    /// updates). No-op for the common furnace-free section.
    pub fn tick_furnaces(
        &mut self,
        smelt: impl Fn(ItemType) -> Option<ItemStack>,
    ) -> Vec<(usize, usize, usize)> {
        let Some(entities) = self.entities.as_deref_mut() else {
            return Vec::new();
        };
        if entities.furnaces.is_empty() {
            return Vec::new();
        }
        let mut changed = false;
        let mut relit = Vec::new();
        // Key order, not map order: `relit` feeds block-update scheduling, and
        // deterministic ticks (the multiplayer contract) forbid HashMap
        // iteration order leaking into it.
        let mut keys: Vec<u16> = entities.furnaces.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let f = entities.furnaces.get_mut(&key).expect("key just listed");
            // The furnace's slots live in the shared container map under the
            // same key (sibling field — disjoint borrow).
            let Some(container) = entities.containers.get_mut(&key) else {
                continue;
            };
            let was_lit = f.is_lit();
            if f.tick(&mut container.slots, &smelt) {
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
        containers: HashMap<u16, Container>,
        entity_facings: HashMap<u16, Facing>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        sapling_stages: HashMap<u16, u8>,
        doors: HashMap<u16, DoorState>,
        stairs: HashMap<u16, StairState>,
        slabs: HashMap<u16, SlabState>,
        log_axes: HashMap<u16, LogAxis>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
    ) -> Self {
        Self::from_shared(
            cx,
            cy,
            cz,
            blocks.into(),
            water.map(Arc::from),
            furnaces,
            containers,
            entity_facings,
            torches,
            model_cells,
            model_facings,
            sapling_stages,
            doors,
            stairs,
            slabs,
            log_axes,
            cell_kv,
            None,
        )
    }

    /// Rebuild a replica section from immutable wire buffers and server-derived
    /// counters. No voxel buffer is copied or scanned on the render thread.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_replica(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: Arc<[u8]>,
        water: Option<Arc<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        containers: HashMap<u16, Container>,
        entity_facings: HashMap<u16, Facing>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        sapling_stages: HashMap<u16, u8>,
        doors: HashMap<u16, DoorState>,
        stairs: HashMap<u16, StairState>,
        slabs: HashMap<u16, SlabState>,
        log_axes: HashMap<u16, LogAxis>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
        metrics: SectionMetrics,
    ) -> Self {
        debug_assert!(metrics.valid());
        Self::from_shared(
            cx,
            cy,
            cz,
            blocks,
            water,
            furnaces,
            containers,
            entity_facings,
            torches,
            model_cells,
            model_facings,
            sapling_stages,
            doors,
            stairs,
            slabs,
            log_axes,
            cell_kv,
            Some(metrics),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_shared(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: Arc<[u8]>,
        water: Option<Arc<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        containers: HashMap<u16, Container>,
        entity_facings: HashMap<u16, Facing>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        sapling_stages: HashMap<u16, u8>,
        doors: HashMap<u16, DoorState>,
        stairs: HashMap<u16, StairState>,
        slabs: HashMap<u16, SlabState>,
        log_axes: HashMap<u16, LogAxis>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
        metrics: Option<SectionMetrics>,
    ) -> Self {
        let entities = BlockEntities {
            furnaces,
            containers,
            entity_facings,
        };
        let mut s = Self {
            cx,
            cy,
            cz,
            blocks,
            states: BlockStates::from_shared(
                water,
                torches,
                model_cells,
                model_facings,
                sapling_stages,
                doors,
                stairs,
                slabs,
                log_axes,
                cell_kv,
            ),
            entities: (!entities.is_empty()).then(|| Box::new(entities)),
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
            particle_emitter_count: 0,
        };
        if let Some(metrics) = metrics {
            s.install_metrics(metrics);
            if s.non_air_count == 0 {
                s.blocks = uniform_cube(0);
            } else if s.water_count as usize == SECTION_VOLUME {
                s.blocks = uniform_cube(Block::Water.id());
            }
        } else {
            s.recompute_opaque_count();
        }
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

    #[test]
    fn particle_emitter_hint_tracks_incremental_and_bulk_blocks() {
        let mut section = Section::new(0, 0, 0);
        assert!(!section.has_particle_emitters());

        section.set_block(1, 1, 1, Block::Stone);
        assert!(!section.has_particle_emitters());

        section.set_block(1, 1, 1, Block::Torch);
        assert!(section.has_particle_emitters());

        section.set_block(1, 1, 1, Block::Air);
        assert!(!section.has_particle_emitters());

        section.blocks_slice_mut()[0] = Block::Torch.id();
        section.recompute_opaque_count();
        assert!(section.has_particle_emitters());
    }
}
