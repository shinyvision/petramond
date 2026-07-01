//! Live surface-density terrain pipeline.
//!
//! This is the Stage 6A bridge from the staged density graph to chunk terrain
//! fill. It owns only surface terrain: climate biome assignment, density-sign
//! ground/air fill, sea-level water, and exposed-run surface dressing. Caves and
//! underground scatter remain external post-surface stages.

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::{idx, section_idx, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL, SECTION_SIZE};
use crate::section::Section;

use super::lattice::{DensityLattice, DensityLatticeBounds, DensityLatticeCellSize};
use super::terrain::{channels, TerrainDensityGraph, TerrainDensitySpec};
use std::collections::HashMap;

use crate::worldgen::biome::climate::{
    BiomeClimateIndex, ClimateAxis, ClimateSampleCell, ClimateSampler, SurfaceClimate,
    CLIMATE_SAMPLE_CELL_X, CLIMATE_SAMPLE_CELL_Z,
};
use crate::worldgen::biome::spec;
use crate::worldgen::proto::ProtoChunk;
use crate::worldgen::region::RegionCells;
use crate::worldgen::surface::rule::SurfaceCtx;
use crate::worldgen::surface::SurfaceSystem;

/// Depth below which every biome surface rule resolves to a single depth-independent
/// block (the deepest `DepthFromTop` band across all biomes is 4; this leaves margin).
/// Below this depth `fill_section` fills a whole section-column with one block.
const MAX_SKIN_BAND_DEPTH: i32 = 8;

const BEACH_MAX_SURFACE_Y: i32 = SEA_LEVEL + 5;
const BEACH_MAX_CONTINENTALITY: f32 = 0.14;
const BEACH_SCAN_RADIUS: i32 = 16;
const BEACH_SCAN_STEP: i32 = 8;

#[derive(Clone, Debug)]
pub(crate) struct SurfaceDensitySystem {
    seed: u32,
    density: TerrainDensityGraph,
    climate: BiomeClimateIndex,
    surface: SurfaceSystem,
}

impl SurfaceDensitySystem {
    pub(crate) fn new(seed: u32) -> Self {
        Self {
            seed,
            density: TerrainDensitySpec::default_surface().build_graph(seed),
            climate: BiomeClimateIndex::default_surface(),
            surface: SurfaceSystem,
        }
    }

    pub(crate) fn biome_at(&self, wx: i32, wz: i32) -> Biome {
        let mut cells = self.climate_cells();
        let surf_y = self.surface_heights(wx, wz, 1, 1)[0];
        self.biome_at_cell(&mut cells, wx, wz, surf_y)
    }

    pub(crate) fn region(&self, x0: i32, z0: i32, w: usize, h: usize) -> RegionCells {
        let surfaces = self.surface_heights(x0, z0, w, h);
        let mut region = RegionCells::new(x0, z0, w, h);
        let mut cells = self.climate_cells();

        for z in 0..h {
            for x in 0..w {
                let wx = x0 + x as i32;
                let wz = z0 + z as i32;
                let i = z * w + x;
                region.surf[i] = surfaces[i];
                region.biomes[i] = self.biome_at_cell(&mut cells, wx, wz, surfaces[i]);
            }
        }

        region
    }

    pub(crate) fn surface_heights(&self, x0: i32, z0: i32, w: usize, h: usize) -> Vec<i32> {
        let bounds = DensityLatticeBounds::new(x0, 0, z0, w, CHUNK_SY, h);
        let lattice = self.master_density_lattice(bounds);
        lattice
            .top_solid_surfaces()
            .into_iter()
            .map(|surf| surf.unwrap_or(-1))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn fill_chunk(&self, proto: &mut ProtoChunk, region: &RegionCells) {
        let bounds = DensityLatticeBounds::chunk(proto.cx(), proto.cz());
        let lattice = self.master_density_lattice(bounds);
        let (ox, oz) = proto.chunk_origin_world();

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let (_, biome) = region.at(wx, wz);
                proto.set_biome(x, z, biome.id());
                self.fill_column(proto.terrain_blocks_mut(), &lattice, x, z, wx, wz, biome);
            }
        }
    }

    pub(crate) fn fill_chunk_direct(&self, proto: &mut ProtoChunk) {
        let bounds = DensityLatticeBounds::chunk(proto.cx(), proto.cz());
        let lattice = self.master_density_lattice(bounds);
        let (ox, oz) = proto.chunk_origin_world();
        let mut cells = self.climate_cells();

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let surf_y = lattice.top_solid_surface(x, z).unwrap_or(-1);
                let biome = self.biome_at_cell(&mut cells, wx, wz, surf_y);
                proto.set_biome(x, z, biome.id());
                self.fill_column(proto.terrain_blocks_mut(), &lattice, x, z, wx, wz, biome);
            }
        }
    }

    /// Cubic terrain fill for one 16³ section, driven by the column's precomputed
    /// biome + density surface (`biomes`/`surf`, the column's 16×16 grids indexed
    /// `z*16 + x`) instead of a per-section density lattice.
    ///
    /// This is byte-identical to [`fill_column`](Self::fill_column) for the section's
    /// slab. `master_density` is depth-only and exactly linear in Y, so a voxel is
    /// solid IFF `wy <= surf`, and its surface depth is exactly `surf - wy` — the same
    /// run-top/`depth_from_top` the lattice walk derives, with no overhangs to track.
    /// Solid voxels take their skin material, non-solid voxels at or below sea level
    /// take water, the rest stay air. Works for ANY `cy` (incl. below y=0, where the
    /// lattice cannot be built): there, every voxel is far below the surface and
    /// resolves through the deep fast path.
    pub(crate) fn fill_section(&self, section: &mut Section, biomes: &[u8], surf: &[i32]) {
        let (ox, oy, oz) = section.origin_world();
        let section_top = oy + SECTION_SIZE as i32 - 1;
        let seed = self.seed;
        let blocks = section.blocks_slice_mut();
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let i = z * SECTION_SIZE + x;
                let s = surf[i];
                let biome = Biome::from_id(biomes[i]);
                let rule = spec(biome).surface;
                let wx = ox + x as i32;
                let wz = oz + z as i32;

                // Deep fast path: the entire section column is solid and below the
                // deepest depth-gated skin band, so every voxel resolves to the SAME
                // (depth-independent) block — compute it once and fill the column.
                if section_top <= s && (s - section_top) > MAX_SKIN_BAND_DEPTH {
                    let deep = self
                        .surface
                        .skin_block(
                            &SurfaceCtx {
                                seed,
                                wx,
                                wz,
                                y: oy,
                                surf_y: s,
                                depth_from_top: (s - oy) as u32,
                            },
                            rule,
                        )
                        .id();
                    for ly in 0..SECTION_SIZE {
                        blocks[section_idx(x, ly, z)] = deep;
                    }
                    continue;
                }

                for ly in 0..SECTION_SIZE {
                    let wy = oy + ly as i32;
                    let id = if wy <= s {
                        let ctx = SurfaceCtx {
                            seed,
                            wx,
                            wz,
                            y: wy,
                            surf_y: s,
                            depth_from_top: (s - wy) as u32,
                        };
                        self.surface.skin_block(&ctx, rule).id()
                    } else if wy <= SEA_LEVEL {
                        Block::Water.id()
                    } else {
                        continue; // air — section starts zeroed.
                    };
                    blocks[section_idx(x, ly, z)] = id;
                }
            }
        }
    }

    fn fill_column(
        &self,
        blocks: &mut [u8],
        lattice: &DensityLattice,
        x: usize,
        z: usize,
        wx: i32,
        wz: i32,
        biome: Biome,
    ) {
        let surface_rule = spec(biome).surface;
        let mut run_top: Option<i32> = None;
        let mut depth_from_top = 0u32;

        for y in (0..CHUNK_SY).rev() {
            let wy = y as i32;
            if !lattice.solid_at_local(x, y, z) {
                run_top = None;
                depth_from_top = 0;
                if wy <= SEA_LEVEL {
                    blocks[idx(x, y, z)] = Block::Water.id();
                }
                continue;
            }

            let surf_y = match run_top {
                Some(top) => top,
                None => {
                    run_top = Some(wy);
                    wy
                }
            };
            let ctx = SurfaceCtx {
                seed: self.seed,
                wx,
                wz,
                y: wy,
                surf_y,
                depth_from_top,
            };
            let block = self.surface.skin_block(&ctx, surface_rule);
            blocks[idx(x, y, z)] = block.id();
            depth_from_top += 1;
        }
    }

    /// Build a fresh per-operation climate-cell cache. Climate is continent-scale
    /// low-frequency noise, so biome classification is memoized per shared 4×4
    /// `ClimateSampleCell`: every column in a cell, and every coast-scan neighbour
    /// that lands in it, reuses one sample+classify instead of recomputing. Output
    /// stays a pure function of `(seed, cell)`, independent of call order.
    fn climate_cells(&self) -> ClimateCellCache<'_> {
        ClimateCellCache::new(ClimateSampler::new(self.density.graph()), &self.climate)
    }

    fn biome_at_cell(
        &self,
        cells: &mut ClimateCellCache<'_>,
        wx: i32,
        wz: i32,
        surf_y: i32,
    ) -> Biome {
        let cell = cells.at(wx, wz);
        self.resolve_coast_biome(cells, wx, wz, surf_y, cell)
    }

    fn resolve_coast_biome(
        &self,
        cells: &mut ClimateCellCache<'_>,
        wx: i32,
        wz: i32,
        surf_y: i32,
        cell: CellClimate,
    ) -> Biome {
        if !can_be_beach_base(cell.base) || surf_y > BEACH_MAX_SURFACE_Y {
            return cell.base;
        }
        let continentality = cell
            .climate
            .get(ClimateAxis::Continentality)
            .expect("surface climate must expose continentality");
        if continentality > BEACH_MAX_CONTINENTALITY {
            return cell.base;
        }
        if self.near_ocean_climate(cells, wx, wz) {
            Biome::Beach
        } else {
            cell.base
        }
    }

    fn near_ocean_climate(&self, cells: &mut ClimateCellCache<'_>, wx: i32, wz: i32) -> bool {
        for dz in (-BEACH_SCAN_RADIUS..=BEACH_SCAN_RADIUS).step_by(BEACH_SCAN_STEP as usize) {
            for dx in (-BEACH_SCAN_RADIUS..=BEACH_SCAN_RADIUS).step_by(BEACH_SCAN_STEP as usize) {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let dist2 = dx * dx + dz * dz;
                if dist2 > BEACH_SCAN_RADIUS * BEACH_SCAN_RADIUS {
                    continue;
                }
                if is_ocean_biome(cells.cell_base(ClimateSampleCell::surface(wx + dx, wz + dz))) {
                    return true;
                }
            }
        }
        false
    }

    fn master_density_lattice(&self, bounds: DensityLatticeBounds) -> DensityLattice {
        DensityLattice::sample_channel(
            self.density.graph(),
            channels::MASTER_DENSITY,
            bounds,
            DensityLatticeCellSize::default(),
        )
        .expect("surface density graph must expose master_density")
    }
}

/// Memoized base climate for one shared 4×4 climate cell: the sampled climate
/// vector plus its classified base biome. Coast/beach derivation still layers the
/// per-column surface height on top, but the expensive noise sample + nearest-rect
/// classification happens once per cell.
#[derive(Copy, Clone)]
struct CellClimate {
    climate: SurfaceClimate,
    base: Biome,
}

struct ClimateCellCache<'a> {
    sampler: ClimateSampler<'a>,
    index: &'a BiomeClimateIndex,
    /// Raw climate sampled once per 4×4 cell corner (the expensive noise step).
    climate: HashMap<ClimateSampleCell, SurfaceClimate>,
    /// Coarse per-cell classification of that corner — only the cheap ocean
    /// proximity scan needs cell-resolution biomes.
    base: HashMap<ClimateSampleCell, Biome>,
}

impl<'a> ClimateCellCache<'a> {
    fn new(sampler: ClimateSampler<'a>, index: &'a BiomeClimateIndex) -> Self {
        Self {
            sampler,
            index,
            climate: HashMap::new(),
            base: HashMap::new(),
        }
    }

    fn cell_climate(&mut self, cell: ClimateSampleCell) -> SurfaceClimate {
        if let Some(cached) = self.climate.get(&cell) {
            return *cached;
        }
        let climate = self
            .sampler
            .sample_surface_cell(cell)
            .expect("surface density graph must expose climate channels")
            .climate;
        self.climate.insert(cell, climate);
        climate
    }

    fn cell_base(&mut self, cell: ClimateSampleCell) -> Biome {
        if let Some(cached) = self.base.get(&cell) {
            return *cached;
        }
        let climate = self.cell_climate(cell);
        let base = self
            .index
            .classify_surface(climate)
            .expect("surface climate index must classify default biomes");
        self.base.insert(cell, base);
        base
    }

    /// Per-column climate, bilinearly interpolated from the four surrounding 4×4
    /// cell corners so biome edges resolve to single blocks instead of 4×4 steps.
    fn climate_at(&mut self, wx: i32, wz: i32) -> SurfaceClimate {
        let cx = wx.div_euclid(CLIMATE_SAMPLE_CELL_X);
        let cz = wz.div_euclid(CLIMATE_SAMPLE_CELL_Z);
        let fx = (wx - cx * CLIMATE_SAMPLE_CELL_X) as f32 / CLIMATE_SAMPLE_CELL_X as f32;
        let fz = (wz - cz * CLIMATE_SAMPLE_CELL_Z) as f32 / CLIMATE_SAMPLE_CELL_Z as f32;
        let c00 = self.cell_climate(ClimateSampleCell::at_surface_indices(cx, cz));
        let c10 = self.cell_climate(ClimateSampleCell::at_surface_indices(cx + 1, cz));
        let c01 = self.cell_climate(ClimateSampleCell::at_surface_indices(cx, cz + 1));
        let c11 = self.cell_climate(ClimateSampleCell::at_surface_indices(cx + 1, cz + 1));
        SurfaceClimate::bilerp(c00, c10, c01, c11, fx, fz)
    }

    fn at(&mut self, wx: i32, wz: i32) -> CellClimate {
        let climate = self.climate_at(wx, wz);
        // Only boundary cells (corners disagreeing on biome) need a per-column
        // classification; a cell whose four corners agree is biome interior, so
        // reuse that biome and skip the nearest-rect query for all 16 columns.
        let base = self.uniform_base(wx, wz).unwrap_or_else(|| {
            self.index
                .classify_surface(climate)
                .expect("surface climate index must classify default biomes")
        });
        CellClimate { climate, base }
    }

    fn uniform_base(&mut self, wx: i32, wz: i32) -> Option<Biome> {
        let cx = wx.div_euclid(CLIMATE_SAMPLE_CELL_X);
        let cz = wz.div_euclid(CLIMATE_SAMPLE_CELL_Z);
        let base = self.cell_base(ClimateSampleCell::at_surface_indices(cx, cz));
        let agree = self.cell_base(ClimateSampleCell::at_surface_indices(cx + 1, cz)) == base
            && self.cell_base(ClimateSampleCell::at_surface_indices(cx, cz + 1)) == base
            && self.cell_base(ClimateSampleCell::at_surface_indices(cx + 1, cz + 1)) == base;
        agree.then_some(base)
    }
}

fn is_ocean_biome(biome: Biome) -> bool {
    matches!(biome, Biome::Ocean | Biome::DeepOcean)
}

fn can_be_beach_base(biome: Biome) -> bool {
    !matches!(
        biome,
        Biome::Ocean
            | Biome::DeepOcean
            | Biome::Beach
            | Biome::Mountains
            | Biome::SnowyPeaks
            | Biome::SnowySlopes
            | Biome::WindsweptHills
            | Biome::StonyPeaks
            | Biome::MountainEdge
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::worldgen::biome::climate::{
        AxisRange, BiomeClimateEntry, ClimateRect, SurfaceClimate,
    };
    use crate::worldgen::graph::{Channel, SamplePoint, SampledScalarField};

    #[derive(Debug)]
    struct PlaneDensity {
        surface_y: f64,
    }

    impl SampledScalarField for PlaneDensity {
        fn sample(&self, point: SamplePoint) -> f64 {
            self.surface_y - point.y
        }
    }

    #[derive(Debug)]
    struct AlternatingRuns;

    impl SampledScalarField for AlternatingRuns {
        fn sample(&self, point: SamplePoint) -> f64 {
            let y = point.y as i32;
            match y {
                72 | 88 => 1.0,
                80 | 96 => -1.0,
                _ => -1.0,
            }
        }
    }

    #[derive(Debug)]
    struct CoastContinentality;

    impl SampledScalarField for CoastContinentality {
        fn sample(&self, point: SamplePoint) -> f64 {
            if point.x < 0.0 {
                -0.5
            } else if point.x <= 16.0 {
                0.05
            } else {
                0.5
            }
        }

        fn depends_on_y(&self) -> bool {
            false
        }
    }

    fn plains_index() -> BiomeClimateIndex {
        const ANY: AxisRange = AxisRange::new(-1.0, 1.0);
        static PLAINS: &[ClimateRect] = &[ClimateRect::surface(ANY, ANY, ANY, ANY, ANY)];
        BiomeClimateIndex::new(&[BiomeClimateEntry {
            biome: Biome::Plains,
            rectangles: PLAINS,
        }])
    }

    fn coast_index() -> BiomeClimateIndex {
        const ANY: AxisRange = AxisRange::new(-1.0, 1.0);
        static OCEAN: &[ClimateRect] = &[ClimateRect::surface(
            ANY,
            ANY,
            AxisRange::new(-1.0, -0.2),
            ANY,
            ANY,
        )];
        static PLAINS: &[ClimateRect] = &[ClimateRect::surface(
            ANY,
            ANY,
            AxisRange::new(0.0, 1.0),
            ANY,
            ANY,
        )];
        BiomeClimateIndex::new(&[
            BiomeClimateEntry {
                biome: Biome::Ocean,
                rectangles: OCEAN,
            },
            BiomeClimateEntry {
                biome: Biome::Plains,
                rectangles: PLAINS,
            },
        ])
    }

    fn test_system(field: impl SampledScalarField + 'static) -> SurfaceDensitySystem {
        let seed = 0x1234_5678;
        let mut density = TerrainDensitySpec::default_surface().build_graph(seed);
        let node = density.graph_mut().sampled_field(field);
        density
            .graph_mut()
            .set_channel(Channel::new(channels::MASTER_DENSITY), node);
        SurfaceDensitySystem {
            seed,
            density,
            climate: plains_index(),
            surface: SurfaceSystem,
        }
    }

    fn coast_system() -> SurfaceDensitySystem {
        let seed = 0x1234_5678;
        let mut density = TerrainDensitySpec::default_surface().build_graph(seed);
        let density_node = density
            .graph_mut()
            .sampled_field(PlaneDensity { surface_y: 65.0 });
        density
            .graph_mut()
            .set_channel(Channel::new(channels::MASTER_DENSITY), density_node);
        let continentality = density.graph_mut().sampled_field(CoastContinentality);
        let zero = density.graph_mut().constant(0.0);
        density
            .graph_mut()
            .set_channel(Channel::new(channels::TEMPERATURE), zero);
        density
            .graph_mut()
            .set_channel(Channel::new(channels::HUMIDITY), zero);
        density
            .graph_mut()
            .set_channel(Channel::new(channels::CONTINENTALITY), continentality);
        density
            .graph_mut()
            .set_channel(Channel::new(channels::EROSION), zero);
        density
            .graph_mut()
            .set_channel(Channel::new(channels::VARIANCE), zero);
        SurfaceDensitySystem {
            seed,
            density,
            climate: coast_index(),
            surface: SurfaceSystem,
        }
    }

    fn generate_surface_chunk(system: &SurfaceDensitySystem, cx: i32, cz: i32) -> Chunk {
        let region = system.region(
            cx * CHUNK_SX as i32,
            cz * CHUNK_SZ as i32,
            CHUNK_SX,
            CHUNK_SZ,
        );
        let mut proto = ProtoChunk::new(cx, cz);
        system.fill_chunk(&mut proto, &region);
        proto.into_chunk()
    }

    fn top_solid_excluding_water(chunk: &Chunk, x: usize, z: usize) -> Option<i32> {
        (0..CHUNK_SY).rev().find_map(|y| {
            let block = chunk.block(x, y, z);
            (block != Block::Air && block != Block::Water).then_some(y as i32)
        })
    }

    fn exposed_solid_run_tops(chunk: &Chunk, x: usize, z: usize) -> Vec<i32> {
        (0..CHUNK_SY)
            .rev()
            .filter_map(|y| {
                let block = chunk.block(x, y, z);
                if block == Block::Air || block == Block::Water {
                    return None;
                }
                let above_open = y + 1 >= CHUNK_SY
                    || matches!(chunk.block(x, y + 1, z), Block::Air | Block::Water);
                above_open.then_some(y as i32)
            })
            .collect()
    }

    /// The deep fast path in `fill_section` requires every biome rule to be
    /// depth-independent below `MAX_SKIN_BAND_DEPTH`, and `SurfaceCond::Underwater`
    /// is a whole-column predicate — a depth-ungated underwater branch skins the
    /// column to bedrock, so cave carving exposes all-sand/all-dirt caves under
    /// water bodies.
    #[test]
    fn deep_skin_is_depth_independent_and_ignores_underwater_status() {
        let surface = SurfaceSystem;
        let deep = (MAX_SKIN_BAND_DEPTH + 1) as u32;
        let ctx = |wx: i32, wz: i32, surf_y: i32, depth: u32| SurfaceCtx {
            seed: 1,
            wx,
            wz,
            y: surf_y - depth as i32,
            surf_y,
            depth_from_top: depth,
        };

        for spec in crate::worldgen::biome::SPECS.iter() {
            for (wx, wz) in [(0, 0), (137, -911), (-4096, 512)] {
                for surf_y in [SEA_LEVEL - 20, SEA_LEVEL + 20, 160] {
                    assert_eq!(
                        surface.skin_block(&ctx(wx, wz, surf_y, deep), spec.surface),
                        surface.skin_block(&ctx(wx, wz, surf_y, deep + 120), spec.surface),
                        "{:?} skin is depth-dependent below MAX_SKIN_BAND_DEPTH \
                         at ({wx},{wz}) surf_y={surf_y}",
                        spec.biome
                    );
                }
                assert_eq!(
                    surface.skin_block(&ctx(wx, wz, SEA_LEVEL - 20, deep), spec.surface),
                    surface.skin_block(&ctx(wx, wz, SEA_LEVEL + 20, deep), spec.surface),
                    "{:?} deep material differs between underwater and dry columns \
                     at ({wx},{wz})",
                    spec.biome
                );
            }
        }
    }

    #[test]
    fn density_sign_fill_produces_solid_air_and_sea_water() {
        let system = test_system(PlaneDensity { surface_y: 60.0 });
        let chunk = generate_surface_chunk(&system, 0, 0);

        assert_ne!(chunk.block(0, 59, 0), Block::Air);
        assert_ne!(chunk.block(0, 59, 0), Block::Water);
        assert_eq!(chunk.block(0, 60, 0), Block::Water);
        assert_eq!(chunk.block(0, SEA_LEVEL as usize, 0), Block::Water);
        assert_eq!(chunk.block(0, SEA_LEVEL as usize + 1, 0), Block::Air);
    }

    #[test]
    fn surface_dressing_resets_across_multiple_solid_runs() {
        let system = test_system(AlternatingRuns);
        let chunk = generate_surface_chunk(&system, 0, 0);
        let run_tops = exposed_solid_run_tops(&chunk, 0, 0);

        assert!(
            run_tops.len() >= 2,
            "test density should produce multiple exposed solid runs"
        );
        for y in run_tops.into_iter().take(2) {
            assert_eq!(chunk.block(0, y as usize, 0), Block::Grass);
        }
    }

    #[test]
    fn region_top_solid_matches_filled_chunk_excluding_water() {
        let system = SurfaceDensitySystem::new(0xCAFE_BABE);
        let region = system.region(0, 0, CHUNK_SX, CHUNK_SZ);
        let mut proto = ProtoChunk::new(0, 0);
        system.fill_chunk(&mut proto, &region);
        let chunk = proto.into_chunk();

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                assert_eq!(
                    top_solid_excluding_water(&chunk, x, z),
                    Some(region.surf[z * CHUNK_SX + x]),
                    "column ({x},{z})"
                );
            }
        }
    }

    #[test]
    fn direct_fill_matches_region_fill() {
        let system = SurfaceDensitySystem::new(7);

        for (cx, cz) in [(0, 0), (-2, 1), (4, -3)] {
            let ox = cx * CHUNK_SX as i32;
            let oz = cz * CHUNK_SZ as i32;
            let region = system.region(ox, oz, CHUNK_SX, CHUNK_SZ);

            let mut region_proto = ProtoChunk::new(cx, cz);
            system.fill_chunk(&mut region_proto, &region);
            let region_chunk = region_proto.into_chunk();

            let mut direct_proto = ProtoChunk::new(cx, cz);
            system.fill_chunk_direct(&mut direct_proto);
            let direct_chunk = direct_proto.into_chunk();

            assert_eq!(
                region_chunk.blocks_slice(),
                direct_chunk.blocks_slice(),
                "blocks differ at ({cx},{cz})"
            );
            assert_eq!(
                region_chunk.biomes_slice(),
                direct_chunk.biomes_slice(),
                "biomes differ at ({cx},{cz})"
            );
        }
    }

    #[test]
    fn biome_assignment_is_stable_across_overlapping_regions() {
        let system = SurfaceDensitySystem::new(7);
        let small = system.region(-8, 3, 16, 16);
        let large = system.region(-16, -5, 40, 32);

        for wz in 3..19 {
            for wx in -8..8 {
                assert_eq!(
                    small.at(wx, wz).1,
                    large.at(wx, wz).1,
                    "biome mismatch at ({wx},{wz})"
                );
            }
        }
    }

    #[test]
    fn fallback_biome_lookup_matches_region_biomes() {
        let system = SurfaceDensitySystem::new(99);
        let region = system.region(-4, -4, 12, 12);

        for wz in -4..8 {
            for wx in -4..8 {
                assert_eq!(
                    system.biome_at(wx, wz),
                    region.at(wx, wz).1,
                    "biome mismatch at ({wx},{wz})"
                );
            }
        }
    }

    #[test]
    fn beach_is_derived_only_on_low_land_near_ocean_climate() {
        let system = coast_system();

        assert_eq!(system.biome_at(-8, 0), Biome::Ocean);
        assert_eq!(system.biome_at(8, 0), Biome::Beach);
        assert_eq!(system.biome_at(40, 0), Biome::Plains);
    }

    #[test]
    fn climate_classification_uses_variance_derived_ridge() {
        let index = plains_index();
        assert_eq!(
            index.classify_surface(SurfaceClimate::new(0.0, 0.0, 0.0, 0.0, 0.25)),
            Some(Biome::Plains)
        );
    }
}
