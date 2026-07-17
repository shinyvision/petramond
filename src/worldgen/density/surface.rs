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

use crate::worldgen::biome::climate::{
    BiomeClimateIndex, ClimateAxis, ClimateSampleCell, ClimateSampler,
};
use crate::worldgen::biome::spec;
use crate::worldgen::biome::surface_table::FROZEN_TEMPERATURE_MAX;
use crate::worldgen::feature::vegetation::patch_field;
use crate::worldgen::proto::ProtoChunk;
use crate::worldgen::region::RegionCells;
use crate::worldgen::surface::rule::SurfaceCtx;
use crate::worldgen::surface::SurfaceSystem;

mod climate_cache;
#[cfg(test)]
mod tests;

use climate_cache::{CellClimate, ClimateCellCache};

/// Depth below which every biome surface rule resolves to a single depth-independent
/// block (the deepest `DepthFromTop` band across all biomes is 4; this leaves margin).
/// Below this depth `fill_section` fills a whole section-column with one block.
const MAX_SKIN_BAND_DEPTH: i32 = 8;

const BEACH_MAX_SURFACE_Y: i32 = SEA_LEVEL + 5;
const BEACH_MAX_CONTINENTALITY: f32 = 0.14;
const BEACH_SCAN_RADIUS: i32 = 16;
const BEACH_SCAN_STEP: i32 = 8;

/// Sea ice: frozen (`temperature < FROZEN_TEMPERATURE_MAX`) shallow water caps
/// its waterline cell with ice — the ice sheet along cold coasts and frozen
/// rivers. `MIN`/`MAX` bound the capped water depth; a low-frequency cluster
/// field picks each column's effective threshold in between, so the sheet's
/// deep-water edge breaks into organic lobes and floes instead of tracing a
/// bathymetry contour.
const SEA_ICE_MIN_DEPTH: i32 = 2;
const SEA_ICE_MAX_DEPTH: i32 = 6;
const SEA_ICE_EDGE_SALT: u64 = 0x0000_5EA1_CE00_0001;
const SEA_ICE_EDGE_PERIOD: f32 = 24.0;

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
        let mut cells = self.climate_cells();

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let (surf_y, biome) = region.at(wx, wz);
                proto.set_biome(x, z, biome.id());
                let waterline = self.waterline_block(&mut cells, wx, wz, surf_y);
                self.fill_column(
                    proto.terrain_blocks_mut(),
                    &lattice,
                    x,
                    z,
                    wx,
                    wz,
                    biome,
                    waterline,
                );
            }
        }
    }

    /// Reference whole-chunk fill via the density lattice — superseded by
    /// [`fill_chunk_from`](Self::fill_chunk_from) in production, kept as the
    /// independent implementation `direct_fill_matches_region_fill` pins the
    /// surf-driven fill against.
    #[cfg(test)]
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
                let waterline = self.waterline_block(&mut cells, wx, wz, surf_y);
                self.fill_column(
                    proto.terrain_blocks_mut(),
                    &lattice,
                    x,
                    z,
                    wx,
                    wz,
                    biome,
                    waterline,
                );
            }
        }
    }

    /// Fill one whole chunk from precomputed per-column `(biome, surf)` — no
    /// density lattice: `master_density` is depth-only and exactly linear in
    /// Y, so a voxel is solid IFF `wy <= surf`, the same equivalence
    /// [`fill_section`](Self::fill_section) relies on. The run below the
    /// deepest depth-gated skin band resolves to one (depth-independent)
    /// block computed once. Byte-identical to [`fill_chunk_direct`], pinned
    /// by `direct_fill_matches_region_fill`.
    pub(crate) fn fill_chunk_from(&self, proto: &mut ProtoChunk, biomes: &[Biome], surf: &[i32]) {
        debug_assert_eq!(biomes.len(), CHUNK_SX * CHUNK_SZ);
        debug_assert_eq!(surf.len(), CHUNK_SX * CHUNK_SZ);
        let (ox, oz) = proto.chunk_origin_world();
        let seed = self.seed;
        // Lazy: only frozen-candidate columns (shallow submerged) touch climate.
        let mut cells: Option<ClimateCellCache<'_>> = None;
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let i = z * CHUNK_SX + x;
                let biome = biomes[i];
                proto.set_biome(x, z, biome.id());
                let s = surf[i];
                let rule = spec(biome).surface;
                let (wx, wz) = (ox + x as i32, oz + z as i32);

                // Water fills non-solid cells at/below sea level (the
                // waterline cell may freeze to sea ice); air stays zeroed.
                let waterline = if s < SEA_LEVEL && SEA_LEVEL - s <= SEA_ICE_MAX_DEPTH {
                    let cells = cells.get_or_insert_with(|| self.climate_cells());
                    self.waterline_block(cells, wx, wz, s)
                } else {
                    Block::Water
                };
                let blocks = proto.terrain_blocks_mut();
                for y in (s + 1).max(0)..=SEA_LEVEL {
                    blocks[idx(x, y as usize, z)] = if y == SEA_LEVEL {
                        waterline.id()
                    } else {
                        Block::Water.id()
                    };
                }
                if s < 0 {
                    continue;
                }
                let top = s.min(CHUNK_SY as i32 - 1);

                // Deep uniform run below the skin band: one skin call fills it.
                let band_lo = (s - MAX_SKIN_BAND_DEPTH).max(0);
                if band_lo > 0 {
                    let deep = self
                        .surface
                        .skin_block(
                            &SurfaceCtx {
                                seed,
                                wx,
                                wz,
                                y: 0,
                                surf_y: s,
                                depth_from_top: s as u32,
                            },
                            rule,
                        )
                        .id();
                    for y in 0..band_lo {
                        blocks[idx(x, y as usize, z)] = deep;
                    }
                }
                for y in band_lo..=top {
                    let ctx = SurfaceCtx {
                        seed,
                        wx,
                        wz,
                        y,
                        surf_y: s,
                        depth_from_top: (s - y) as u32,
                    };
                    blocks[idx(x, y as usize, z)] = self.surface.skin_block(&ctx, rule).id();
                }
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
        // Lazy: only the waterline section's frozen-candidate columns touch climate.
        let mut cells: Option<ClimateCellCache<'_>> = None;
        let holds_waterline = oy <= SEA_LEVEL && SEA_LEVEL <= section_top;
        let blocks = section.blocks_slice_mut();
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let i = z * SECTION_SIZE + x;
                let s = surf[i];
                let biome = Biome::from_id(biomes[i]);
                let rule = spec(biome).surface;
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let waterline =
                    if holds_waterline && s < SEA_LEVEL && SEA_LEVEL - s <= SEA_ICE_MAX_DEPTH {
                        let cells = cells.get_or_insert_with(|| self.climate_cells());
                        self.waterline_block(cells, wx, wz, s)
                    } else {
                        Block::Water
                    };

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
                    } else if wy == SEA_LEVEL {
                        waterline.id()
                    } else if wy < SEA_LEVEL {
                        Block::Water.id()
                    } else {
                        continue; // air — section starts zeroed.
                    };
                    blocks[section_idx(x, ly, z)] = id;
                }
            }
        }
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn fill_column(
        &self,
        blocks: &mut [u8],
        lattice: &DensityLattice,
        x: usize,
        z: usize,
        wx: i32,
        wz: i32,
        biome: Biome,
        waterline: Block,
    ) {
        let surface_rule = spec(biome).surface;
        let mut run_top: Option<i32> = None;
        let mut depth_from_top = 0u32;

        for y in (0..CHUNK_SY).rev() {
            let wy = y as i32;
            if !lattice.solid_at_local(x, y, z) {
                run_top = None;
                depth_from_top = 0;
                if wy == SEA_LEVEL {
                    blocks[idx(x, y, z)] = waterline.id();
                } else if wy < SEA_LEVEL {
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
        ClimateCellCache::new(ClimateSampler::new(self.density.graph()), &self.climate, self.seed)
    }

    /// The block a submerged column's waterline cell (`y == SEA_LEVEL`) takes:
    /// ice over frozen shallow water, else water. A pure per-column function of
    /// `(seed, wx, wz, surf)` — every fill path routes its waterline cell
    /// through this one rule, so they stay byte-identical.
    fn waterline_block(
        &self,
        cells: &mut ClimateCellCache<'_>,
        wx: i32,
        wz: i32,
        surf: i32,
    ) -> Block {
        // Not submerged (or too deep): plain water. The lower bound lives HERE,
        // not only in the callers' fast-path gates, so the rule alone is
        // correct for any caller — a frozen land column must never resolve to
        // ice at a sub-surface pocket.
        let depth = SEA_LEVEL - surf;
        if !(1..=SEA_ICE_MAX_DEPTH).contains(&depth) {
            return Block::Water;
        }
        // The same bilinear per-column temperature the biome classifier reads,
        // so the ice sheet and the snowy shoreline agree on where winter is.
        let temperature = cells
            .climate_at(wx, wz)
            .get(ClimateAxis::Temperature)
            .unwrap_or(0.0);
        if temperature >= FROZEN_TEMPERATURE_MAX {
            return Block::Water;
        }
        let field = patch_field(self.seed, SEA_ICE_EDGE_SALT, wx, wz, SEA_ICE_EDGE_PERIOD);
        let threshold =
            SEA_ICE_MIN_DEPTH + (field * (SEA_ICE_MAX_DEPTH - SEA_ICE_MIN_DEPTH + 1) as f32) as i32;
        if depth <= threshold {
            Block::Ice
        } else {
            Block::Water
        }
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
