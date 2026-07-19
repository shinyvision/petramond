use crate::block::{Block, BlockTag};
use crate::chunk::{SECTION_SIZE, SECTION_VOLUME};

use super::{uniform_cube, Section, SectionMetrics, SectionSummary};

const MB_RANDOM_TICK: u8 = 1 << 0;
const MB_OPAQUE: u8 = 1 << 1;
const MB_NON_AIR: u8 = 1 << 2;
const MB_WATER: u8 = 1 << 3;
const MB_BIOME_TINT: u8 = 1 << 4;
const MB_PARTICLE_EMITTER: u8 = 1 << 5;
const MB_LIGHT_EMITTER: u8 = 1 << 6;

/// Per-id metrics class bits, derived once from the SAME predicates the
/// incremental setter path (`adjust_random_tick_count` / `adjust_opaque_count`)
/// uses — the block registry loads exactly once per process, so this can never
/// go stale. Ids beyond the registry read as `Air` through `Block::from_id`,
/// matching the per-cell predicates on such ids.
fn metrics_bits() -> &'static [u8; 256] {
    static BITS: std::sync::LazyLock<[u8; 256]> = std::sync::LazyLock::new(|| {
        let mut bits = [0u8; 256];
        for (i, b) in bits.iter_mut().enumerate() {
            let id = i as u8;
            let block = Block::from_id(id);
            *b = (block.has_random_tick() as u8) * MB_RANDOM_TICK
                | (block.is_opaque() as u8) * MB_OPAQUE
                | ((id != 0) as u8) * MB_NON_AIR
                | ((id == Block::Water.id()) as u8) * MB_WATER
                | (Section::id_uses_biome_tint(id) as u8) * MB_BIOME_TINT
                | (Section::id_has_particle_emitter(id) as u8) * MB_PARTICLE_EMITTER
                | (Section::id_emits_light(id) as u8) * MB_LIGHT_EMITTER;
        }
        bits
    });
    &BITS
}

impl Section {
    // --- Random-tick gate -------------------------------------------------------

    /// Keep [`random_tick_count`](Self::random_tick_count) in step with one cell
    /// changing from `old_id` to `new_id`.
    #[inline]
    pub(super) fn adjust_random_tick_count(&mut self, old_id: u8, new_id: u8) {
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
    pub(super) fn adjust_opaque_count(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        old_id: u8,
        new_id: u8,
    ) {
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
        match (Self::id_emits_light(old_id), Self::id_emits_light(new_id)) {
            (false, true) => self.light_emitter_count += 1,
            (true, false) => self.light_emitter_count -= 1,
            _ => {}
        }
    }

    /// Compute every block-derived counter for a bulk-filled buffer.
    ///
    /// Runs on every generated/loaded section, so it avoids per-cell block
    /// dispatch: one id histogram pass over the 4096 cells, folded through the
    /// per-id [`metrics_bits`] class table (derived from the same predicates
    /// the incremental setters use, so the two paths cannot disagree), then a
    /// boundary-plane pass for `plane_opaque`.
    pub(crate) fn metrics_from_blocks(blocks: &[u8]) -> SectionMetrics {
        if blocks.len() != SECTION_VOLUME {
            return SectionMetrics::default();
        }
        let mut hist = [0u16; 256];
        for &id in blocks {
            hist[id as usize] += 1;
        }
        let bits = metrics_bits();
        let mut out = SectionMetrics::default();
        for (id, &n) in hist.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let n = n as u32;
            let b = bits[id];
            if b & MB_RANDOM_TICK != 0 {
                out.random_tick_count += n;
            }
            if b & MB_OPAQUE != 0 {
                out.opaque_count += n;
            }
            if b & MB_NON_AIR != 0 {
                out.non_air_count += n;
            }
            if b & MB_WATER != 0 {
                out.water_count += n;
            }
            if b & MB_BIOME_TINT != 0 {
                out.biome_tint_count += n;
            }
            if b & MB_PARTICLE_EMITTER != 0 {
                out.particle_emitter_count += n;
            }
            if b & MB_LIGHT_EMITTER != 0 {
                out.light_emitter_count += n;
            }
        }
        if out.opaque_count > 0 {
            let opaque = |x: usize, y: usize, z: usize| {
                (bits[blocks[crate::chunk::section_idx(x, y, z)] as usize] & MB_OPAQUE != 0) as u16
            };
            let hi = SECTION_SIZE - 1;
            for a in 0..SECTION_SIZE {
                for b in 0..SECTION_SIZE {
                    out.plane_opaque[0] += opaque(hi, a, b);
                    out.plane_opaque[1] += opaque(0, a, b);
                    out.plane_opaque[2] += opaque(a, hi, b);
                    out.plane_opaque[3] += opaque(a, 0, b);
                    out.plane_opaque[4] += opaque(a, b, hi);
                    out.plane_opaque[5] += opaque(a, b, 0);
                }
            }
        }
        out
    }

    pub(super) fn install_metrics(&mut self, metrics: SectionMetrics) {
        self.random_tick_count = metrics.random_tick_count;
        self.opaque_count = metrics.opaque_count;
        self.plane_opaque = metrics.plane_opaque;
        self.non_air_count = metrics.non_air_count;
        self.water_count = metrics.water_count;
        self.biome_tint_count = metrics.biome_tint_count;
        self.particle_emitter_count = metrics.particle_emitter_count;
        self.light_emitter_count = metrics.light_emitter_count;
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
            light_emitter_count: self.light_emitter_count,
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

    /// Whether this section holds any block-LIGHT-emitting cell (row
    /// `emission > 0`) — the gate that keeps the light flood's emitter gather
    /// from scanning emitter-free sections.
    #[inline]
    pub fn has_light_emitters(&self) -> bool {
        self.light_emitter_count > 0
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

    #[inline]
    fn id_emits_light(id: u8) -> bool {
        Block::from_id(id).light_emission() > 0
    }

    #[cfg(all(test, feature = "worldgen-tests"))]
    pub(crate) fn random_tick_count(&self) -> u32 {
        self.random_tick_count
    }
}
