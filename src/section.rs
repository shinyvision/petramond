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

use crate::block::Block;
use crate::block_state::BlockStates;
use crate::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::container::Container;
use crate::facing::Facing;
use crate::furnace::Furnace;

mod block_entities;
mod cell_states;
mod metrics;
mod restore;

#[cfg(test)]
mod tests;

crate::wire_enum::wire_enum! {
    // The byte form rides the wire (`ColumnPayload::summaries`); the `Unknown`
    // fallback is the conservative "reads lie" answer.
    pub enum SectionSummary: u8 {
        Unknown = 0,
        Empty = 1,
        FullOpaque = 2,
        FullWater = 3,
        Mixed = 4,
    }
    default Unknown
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
    pub light_emitter_count: u32,
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
            && self.light_emitter_count <= volume
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
    /// This section's light cubes are the UNTOUCHED persisted bake seeded at
    /// decode — no live bake or invalidation has touched them since load.
    /// Records only persist light captured in a globally settled state, so all
    /// such cubes (and all persisted content) are mutually consistent: a
    /// sky-cover change sourced from another persisted record landing cannot
    /// stale them (see the streamer's cover-change invalidation). Cleared by
    /// [`mark_light_dirty`](Self::mark_light_dirty).
    pub light_from_persist: bool,
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
    /// Count of cells whose block row EMITS block light (`emission > 0` —
    /// torches, lit furnaces, pack glow blocks). `0` lets the light flood's
    /// emitter gather skip this section without scanning its cells.
    light_emitter_count: u32,
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
            light_from_persist: false,
            light_revision: 0,
            mesh_revision: 0,
            random_tick_count: 0,
            opaque_count: 0,
            plane_opaque: [0; 6],
            non_air_count: 0,
            water_count: 0,
            biome_tint_count: 0,
            particle_emitter_count: 0,
            light_emitter_count: 0,
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

    /// Bulk water-flow metadata for saving (`None` when nothing is mid-flow —
    /// the buffer is dropped once the last cell settles).
    pub fn water_slice(&self) -> Option<&[u8]> {
        self.states.water_slice()
    }

    /// Whether any water cell is mid-flow (nonzero flow meta). O(1) from the
    /// states counter; the streamed-water kick uses it to skip settled sections
    /// without scanning the 4 KiB meta buffer.
    #[inline]
    pub fn has_flowing_water(&self) -> bool {
        self.states.has_flowing()
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
        self.light_from_persist = false;
        self.light_revision = self.light_revision.wrapping_add(1);
    }

    /// Mark light final as-is WITHOUT installing cubes. Two legitimate
    /// callers: the replica's authoritative-delta ingest (light is
    /// server-owned there; keep sampling the old cubes until the server's
    /// rebake arrives as `LightData`), and a landed rebake whose cubes proved
    /// byte-identical to the cached ones (nothing to install or republish).
    /// Local predicted edits instead install their disposable revision-gated
    /// relight, which can never override server light.
    pub fn mark_light_clean(&mut self) {
        self.light_dirty = false;
    }

    /// Drop the block-light cubes (a rebake that reaches no emitter ships no
    /// buffer; absent reads as all-zero — see [`Self::set_blocklight`]).
    pub fn clear_blocklight(&mut self) {
        self.blocklight = None;
    }
}
