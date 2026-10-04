use crate::block::Block;
use crate::chunk::{SECTION_SIZE, SECTION_VOLUME};
use crate::tile::TileTint;

use super::{Section, SectionMetrics, SectionSummary};

const MB_RANDOM_TICK: u16 = 1 << 0;
const MB_OPAQUE: u16 = 1 << 1;
const MB_NON_AIR: u16 = 1 << 2;
const MB_WATER: u16 = 1 << 3;
const MB_BIOME_TINT: u16 = 1 << 4;
const MB_PRESENTED: u16 = 1 << 5;
const MB_LIGHT_EMITTER: u16 = 1 << 6;
const MB_FLUID: u16 = 1 << 7;
const MB_QUENCHES: u16 = 1 << 8;
const MB_QUENCHER: u16 = 1 << 9;

const LOW_HIST: usize = 256;

const fn bit(flag: u16) -> usize {
    flag.trailing_zeros() as usize
}

/// Block-class changes gathered over a batch of cell writes, folded into a section's counters at
/// once by [`Section::apply_metrics`].
#[derive(Default)]
pub(super) struct MetricTally {
    counts: [i32; 16],
    planes: [i32; 6],
    emitters: Vec<(u16, bool)>,
}

impl MetricTally {
    /// Cell `i` changing from class bits `old` to `new`.
    #[inline]
    pub(super) fn note(&mut self, i: usize, old: u16, new: u16) {
        let changed = old ^ new;
        if changed == 0 {
            return;
        }
        let mut bits = changed;
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            self.counts[b] += if new >> b & 1 != 0 { 1 } else { -1 };
        }
        if changed & MB_OPAQUE != 0 {
            let d = if new & MB_OPAQUE != 0 { 1 } else { -1 };
            let (x, y, z) = crate::chunk::section_local(i);
            let hi = SECTION_SIZE - 1;
            for (plane, on) in [x == hi, x == 0, y == hi, y == 0, z == hi, z == 0]
                .into_iter()
                .enumerate()
            {
                if on {
                    self.planes[plane] += d;
                }
            }
        }
        if changed & MB_PRESENTED != 0 {
            self.emitters.push((i as u16, new & MB_PRESENTED != 0));
        }
    }
}

pub(crate) static METRICS: crate::content::Slot<Box<[u16]>> = crate::content::Slot::new(
    crate::content::stage::SECTION_METRICS,
    &[
        crate::content::stage::BLOCKS,
        crate::content::stage::PARTICLE_EMITTERS,
    ],
    derive_metrics_bits,
);

fn derive_metrics_bits(_: &crate::content::ContentRegistry) -> Result<Box<[u16]>, String> {
    let quench = |block: Block| block.fluid_def().and_then(|f| f.quench);
    let mut bits = vec![0u16; Block::all().len()].into_boxed_slice();
    for &block in Block::all() {
        if let Some(q) = quench(block) {
            bits[q.by.id() as usize] |= MB_QUENCHER;
        }
    }
    for (i, b) in bits.iter_mut().enumerate() {
        let id = i as u16;
        let block = Block::from_id(id);
        *b |= (u16::from(block.has_random_tick()) * MB_RANDOM_TICK)
            | (u16::from(block.is_opaque()) * MB_OPAQUE)
            | (u16::from(id != 0) * MB_NON_AIR)
            | (u16::from(id == Block::Water.id()) * MB_WATER)
            | (u16::from(block.is_fluid()) * MB_FLUID)
            | (u16::from(quench(block).is_some()) * MB_QUENCHES)
            | (u16::from(Section::id_uses_biome_tint(id)) * MB_BIOME_TINT)
            | (u16::from(Section::id_is_presented(id)) * MB_PRESENTED)
            | (u16::from(Section::id_emits_light(id)) * MB_LIGHT_EMITTER);
    }
    Ok(bits)
}

fn metrics_bits() -> &'static [u16] {
    METRICS.current()
}

impl Section {
    pub fn recompute_random_tick_count(&mut self) {
        self.random_tick_count = self
            .blocks
            .iter()
            .filter(|&id| Block::from_id(id).has_random_tick())
            .count() as u32;
    }

    /// Adjusts every block-derived counter for the cell at `(x, y, z)` changing from `old_id` to
    /// `new_id`, from the same per-id class table the bulk recount folds through.
    #[inline]
    pub(super) fn adjust_metrics(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        old_id: u16,
        new_id: u16,
    ) {
        let bits = metrics_bits();
        let of = |id: u16| bits.get(id as usize).copied().unwrap_or(0);
        self.adjust_metrics_bits(x, y, z, of(old_id), of(new_id));
    }

    #[inline]
    pub(super) fn adjust_metrics_bits(&mut self, x: usize, y: usize, z: usize, old: u16, new: u16) {
        if old != new {
            let mut tally = MetricTally::default();
            tally.note(crate::chunk::section_idx(x, y, z), old, new);
            self.apply_metrics(tally);
        }
    }

    /// Folds a batch's [`MetricTally`] into the block-derived counters.
    pub(super) fn apply_metrics(&mut self, tally: MetricTally) {
        let add = |count: &mut u32, d: i32| *count = count.wrapping_add_signed(d);
        let t = &tally.counts;
        add(&mut self.random_tick_count, t[bit(MB_RANDOM_TICK)]);
        add(&mut self.opaque_count, t[bit(MB_OPAQUE)]);
        add(&mut self.non_air_count, t[bit(MB_NON_AIR)]);
        add(&mut self.water_count, t[bit(MB_WATER)]);
        add(&mut self.fluid_count, t[bit(MB_FLUID)]);
        add(&mut self.quench_count, t[bit(MB_QUENCHES)]);
        add(&mut self.quencher_count, t[bit(MB_QUENCHER)]);
        add(&mut self.biome_tint_count, t[bit(MB_BIOME_TINT)]);
        add(&mut self.light_emitter_count, t[bit(MB_LIGHT_EMITTER)]);
        for (plane, d) in self.plane_opaque.iter_mut().zip(tally.planes) {
            *plane = (*plane as i32 + d) as u16;
        }
        for (cell, on) in tally.emitters {
            match (self.presented_cells.binary_search(&cell), on) {
                (Err(at), true) => self.presented_cells.insert(at, cell),
                (Ok(at), false) => {
                    self.presented_cells.remove(at);
                }
                _ => {}
            }
        }
    }

    /// The per-id class table [`Section::adjust_metrics_bits`] reads, fetched once for a batch.
    #[inline]
    pub(super) fn metric_table() -> &'static [u16] {
        metrics_bits()
    }

    /// Compute every block-derived counter for a bulk-filled buffer.
    ///
    /// Runs on every generated/loaded section, so no per-cell block dispatch: one
    /// histogram pass over the 4096 cells, folded through the per-id `metrics_bits`
    /// class table (same predicates the incremental setters use, so the two paths
    /// can't disagree), then a boundary-plane pass for `plane_opaque`.
    pub fn metrics_from_blocks(blocks: &[u16]) -> SectionMetrics {
        Self::metrics_from(blocks.len(), |i| blocks[i])
    }

    pub fn metrics_from_cube(cube: &super::BlockCube) -> SectionMetrics {
        if cube.len() != SECTION_VOLUME {
            return SectionMetrics::default();
        }
        // One widening copy instead of a representation match per cell.
        let mut ids = [0u16; SECTION_VOLUME];
        cube.copy_ids(&mut ids);
        Self::metrics_from_blocks(&ids)
    }

    fn metrics_from(len: usize, at: impl Fn(usize) -> u16) -> SectionMetrics {
        if len != SECTION_VOLUME {
            return SectionMetrics::default();
        }
        let mut hist = [0u16; LOW_HIST];
        let mut high: Vec<(u16, u32)> = Vec::new();
        for id in (0..len).map(&at) {
            match hist.get_mut(id as usize) {
                Some(slot) => *slot += 1,
                None => match high.iter_mut().find(|(i, _)| *i == id) {
                    Some((_, n)) => *n += 1,
                    None => high.push((id, 1)),
                },
            }
        }
        let bits = metrics_bits();
        let mut out = SectionMetrics::default();
        let tally = hist
            .iter()
            .enumerate()
            .map(|(id, &n)| (id as u16, n as u32))
            .chain(high.iter().copied());
        for (id, n) in tally {
            if n == 0 {
                continue;
            }
            let b = bits.get(id as usize).copied().unwrap_or(0);
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
            if b & MB_FLUID != 0 {
                out.fluid_count += n;
            }
            if b & MB_QUENCHES != 0 {
                out.quench_count += n;
            }
            if b & MB_QUENCHER != 0 {
                out.quencher_count += n;
            }
            if b & MB_BIOME_TINT != 0 {
                out.biome_tint_count += n;
            }
            if b & MB_PRESENTED != 0 {
                out.presented_count += n;
            }
            if b & MB_LIGHT_EMITTER != 0 {
                out.light_emitter_count += n;
            }
        }
        if out.opaque_count > 0 {
            let opaque = |x: usize, y: usize, z: usize| {
                let id = at(crate::chunk::section_idx(x, y, z)) as usize;
                (bits.get(id).copied().unwrap_or(0) & MB_OPAQUE != 0) as u16
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
        self.fluid_count = metrics.fluid_count;
        self.quench_count = metrics.quench_count;
        self.quencher_count = metrics.quencher_count;
        self.biome_tint_count = metrics.biome_tint_count;
        self.light_emitter_count = metrics.light_emitter_count;
        let mut cells = std::mem::take(&mut self.presented_cells);
        cells.clear();
        if metrics.presented_count > 0 {
            cells.reserve(metrics.presented_count as usize);
            cells.extend(
                self.blocks
                    .iter()
                    .enumerate()
                    .filter(|&(_, id)| Self::id_is_presented(id))
                    .map(|(i, _)| i as u16),
            );
        }
        self.presented_cells = cells;
    }

    pub fn stream_metrics(&self) -> SectionMetrics {
        SectionMetrics {
            random_tick_count: self.random_tick_count,
            opaque_count: self.opaque_count,
            plane_opaque: self.plane_opaque,
            non_air_count: self.non_air_count,
            water_count: self.water_count,
            fluid_count: self.fluid_count,
            quench_count: self.quench_count,
            quencher_count: self.quencher_count,
            biome_tint_count: self.biome_tint_count,
            presented_count: self.presented_cells.len() as u32,
            light_emitter_count: self.light_emitter_count,
        }
    }

    pub fn recompute_opaque_count(&mut self) {
        self.install_metrics(Self::metrics_from_cube(&self.blocks));
        self.compact_uniform_blocks();
        self.present = super::IdSet::of_cube(&self.blocks);
    }

    fn compact_uniform_blocks(&mut self) {
        let uniform_id = if self.non_air_count == 0 {
            Some(0u16)
        } else if self.opaque_count as usize == SECTION_VOLUME
            || self.water_count as usize == SECTION_VOLUME
        {
            let first = self.blocks.get(0);
            self.blocks.iter().all(|b| b == first).then_some(first)
        } else {
            None
        };
        if let Some(id) = uniform_id {
            self.blocks.fill(id);
        }
    }

    #[inline]
    pub fn all_opaque(&self) -> bool {
        self.opaque_count as usize == SECTION_VOLUME
    }

    #[inline]
    pub fn has_opaque_blocks(&self) -> bool {
        self.opaque_count > 0
    }

    #[inline]
    pub fn is_empty_air(&self) -> bool {
        self.non_air_count == 0
    }

    #[inline]
    pub fn face_plane_fully_opaque(&self, dx: i32, dy: i32, dz: i32) -> bool {
        const PLANE_AREA: u16 = (SECTION_SIZE * SECTION_SIZE) as u16;
        self.plane_opaque[Self::plane_index(dx, dy, dz)] == PLANE_AREA
    }

    #[inline]
    pub fn face_plane_open_cells(&self, dx: i32, dy: i32, dz: i32) -> u16 {
        (SECTION_SIZE * SECTION_SIZE) as u16 - self.plane_opaque[Self::plane_index(dx, dy, dz)]
    }

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

    #[inline]
    pub fn has_fluid(&self) -> bool {
        self.fluid_count > 0
    }

    #[inline]
    pub fn may_quench_against(&self, other: &Section) -> bool {
        (self.quench_count > 0 && other.quencher_count > 0)
            || (other.quench_count > 0 && self.quencher_count > 0)
    }

    #[inline]
    pub fn has_biome_tint_blocks(&self) -> bool {
        self.biome_tint_count > 0
    }

    #[inline]
    pub fn has_presented_cells(&self) -> bool {
        !self.presented_cells.is_empty()
    }

    #[inline]
    pub fn presented_cells(&self) -> &[u16] {
        &self.presented_cells
    }

    #[inline]
    pub fn has_light_emitters(&self) -> bool {
        self.light_emitter_count > 0
    }

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

    #[inline]
    pub fn has_random_tickable(&self) -> bool {
        self.random_tick_count > 0
    }

    fn id_uses_biome_tint(id: u16) -> bool {
        if id == Block::Air.id() {
            return false;
        }
        let block = Block::from_id(id);
        let overlay = block.side_overlay();
        block
            .tiles()
            .into_iter()
            .chain(overlay.map(|o| o.base))
            .chain(overlay.map(|o| o.overlay))
            .chain(block.covered_side())
            .chain(block.front_tile())
            .chain(block.is_fluid().then(|| block.fluid_flow_tile()))
            .any(|tile| {
                tile.world_tint()
                    .is_some_and(|t| !matches!(t, TileTint::Fixed(_)))
            })
    }

    /// Cells whose block carries client-side presentation of its own (looping
    /// particles, cloth), so the client finds them without scanning sections.
    #[inline]
    fn id_is_presented(id: u16) -> bool {
        let block = Block::from_id(id);
        block.particle_emitter().is_some() || block.cloth().is_some()
    }

    #[inline]
    fn id_emits_light(id: u16) -> bool {
        Block::from_id(id).light_emission() > 0
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn random_tick_count(&self) -> u32 {
        self.random_tick_count
    }
}
