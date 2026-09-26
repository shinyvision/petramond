//! Live surface-density terrain pipeline.
//!
//! This is the Stage 6A bridge from the staged density graph to chunk terrain
//! fill. It owns only surface terrain: climate biome assignment, density-sign
//! ground/air fill, sea-level water, and exposed-run surface dressing. Caves and
//! underground scatter remain external post-surface stages.

use petramond_world::biome::Biome;
use petramond_world::block::Block;
use petramond_world::chunk::{section_idx, SEA_LEVEL, SECTION_SIZE, WORLD_MAX_Y};
use petramond_world::section::Section;

use super::lattice::{DensityLattice, DensityLatticeBounds, DensityLatticeCellSize};
use super::terrain::{channels, FloorDensitySpec, TerrainDensityGraph};

use crate::biome::climate::{
    BiomeClimateIndex, ClimateAxis, ClimateSampleCell, ClimateSampler, CLIMATE_SAMPLE_CELL_X,
    CLIMATE_SAMPLE_CELL_Z,
};
use crate::biome::spec;
use crate::rng::patch_field;
use crate::region::RegionCells;
use crate::surface::rule::SurfaceCtx;
use crate::surface::SurfaceSystem;

mod climate_cache;
#[cfg(test)]
mod tests;

use climate_cache::{CellClimate, ClimateCellCache};

use crate::surface::MAX_SKIN_BAND_DEPTH;

const BEACH_MAX_SURFACE_Y: i32 = SEA_LEVEL + 5;
const BEACH_MAX_CONTINENTALITY: f32 = 0.14;
const BEACH_SCAN_RADIUS: i32 = 16;
const BEACH_SCAN_STEP: i32 = 8;

/// Sea ice: frozen (temperature below the top of the climate table's `frozen`
/// band) shallow water caps its waterline cell with ice — the ice sheet along
/// cold coasts and frozen rivers. `MIN`/`MAX` bound the capped water depth; a low-frequency cluster
/// field picks each column's effective threshold in between, so the sheet's
/// deep-water edge breaks into organic lobes and floes instead of tracing a
/// bathymetry contour.
const SEA_ICE_MIN_DEPTH: i32 = 2;
const SEA_ICE_MAX_DEPTH: i32 = 6;
const SEA_ICE_EDGE_PERIOD: f32 = 24.0;

#[derive(Clone, Debug)]
pub struct SurfaceDensitySystem {
    seed: u32,
    /// Shared with every other system of this world (see `SeedSources`).
    density: std::sync::Arc<TerrainDensityGraph>,
    climate: &'static BiomeClimateIndex,
    surface: SurfaceSystem,
}

impl SurfaceDensitySystem {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            density: crate::noise::sources::SeedSources::for_seed(seed).terrain,
            climate: BiomeClimateIndex::default_surface(),
            surface: SurfaceSystem,
        }
    }

    pub fn biome_at(&self, wx: i32, wz: i32) -> Biome {
        let mut cells = self.climate_cells();
        let surf_y = self.surface_heights(wx, wz, 1, 1)[0];
        self.biome_at_cell(&mut cells, wx, wz, surf_y)
    }

    pub fn region(&self, x0: i32, z0: i32, w: usize, h: usize) -> RegionCells {
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

    pub fn surface_heights(&self, x0: i32, z0: i32, w: usize, h: usize) -> Vec<i32> {
        surface_heights(&self.density, x0, z0, w, h)
    }

    /// Cubic terrain fill for one 16³ section, driven by the column's precomputed
    /// biome + density surface (`biomes`/`surf`, the column's 16×16 grids indexed
    /// `z*16 + x`) instead of a per-section density lattice.
    ///
    /// `master_density` is depth-only and exactly linear in Y, so a voxel is
    /// solid IFF `wy <= surf`, and its surface depth is exactly `surf - wy` — the same
    /// run-top/`depth_from_top` the lattice walk derives, with no overhangs to track.
    /// Solid voxels take their skin material, non-solid voxels at or below sea level
    /// take water, the rest stay air — byte-identical to walking the density lattice
    /// (pinned by `section_fill_matches_the_lattice_reference`). Works for ANY `cy`
    /// (incl. below the surface search floor): there, every voxel is far below
    /// the surface and resolves through the deep fast path.
    pub fn fill_section(&self, section: &mut Section, biomes: &[u8], surf: &[i32]) {
        let (ox, oy, oz) = section.origin_world();
        let section_top = oy + SECTION_SIZE as i32 - 1;
        let seed = self.seed;
        // Lazy: only the waterline section's frozen-candidate columns touch climate.
        let mut cells: Option<ClimateCellCache<'_>> = None;
        let holds_waterline = oy <= SEA_LEVEL && SEA_LEVEL <= section_top;
        section.edit_ids_bulk(|blocks| {
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
        });
    }

    fn climate_cells(&self) -> ClimateCellCache<'_> {
        ClimateCellCache::new(
            ClimateSampler::new(self.density.graph()),
            self.climate,
            self.seed,
        )
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
        if temperature >= crate::data::climate_table::table().frozen_temperature_max {
            return Block::Water;
        }
        let field = patch_field(self.seed, crate::salts::SEA_ICE_EDGE, wx, wz, SEA_ICE_EDGE_PERIOD);
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
            Biome::BEACH
        } else {
            cell.base
        }
    }

    fn near_ocean_climate(&self, cells: &mut ClimateCellCache<'_>, wx: i32, wz: i32) -> bool {
        // Every probe offset is a multiple of the 4-block climate cell size, so
        // `surface(wx+dx, wz+dz)` resolves to `query cell + offset/cell` — the
        // scan's answer is exactly a function of the query's climate cell, and
        // the cache memoizes it per cell (every column of a cell, and the whole
        // shoreline band revisiting it, reuses one scan).
        const {
            assert!(BEACH_SCAN_STEP % CLIMATE_SAMPLE_CELL_X == 0);
            assert!(BEACH_SCAN_RADIUS % CLIMATE_SAMPLE_CELL_X == 0);
            assert!(CLIMATE_SAMPLE_CELL_X == CLIMATE_SAMPLE_CELL_Z);
        }
        cells.near_ocean_memo(wx, wz, |cells, wx, wz| {
            for dz in (-BEACH_SCAN_RADIUS..=BEACH_SCAN_RADIUS).step_by(BEACH_SCAN_STEP as usize) {
                for dx in (-BEACH_SCAN_RADIUS..=BEACH_SCAN_RADIUS).step_by(BEACH_SCAN_STEP as usize)
                {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let dist2 = dx * dx + dz * dz;
                    if dist2 > BEACH_SCAN_RADIUS * BEACH_SCAN_RADIUS {
                        continue;
                    }
                    if is_ocean_biome(cells.cell_base(ClimateSampleCell::surface(wx + dx, wz + dz)))
                    {
                        return true;
                    }
                }
            }
            false
        })
    }
}

/// The world Y range a column's surface is searched in, bottom inclusive and
/// top exclusive: from the terrain density's floor — at and under which the
/// density is solid by construction, so no surface lies lower — to the top of
/// the cubic world. A deeper or taller world moves these bounds, nothing else.
pub(crate) const SURFACE_SEARCH_Y: std::ops::Range<i32> =
    FloorDensitySpec::default_surface().floor_y as i32..WORLD_MAX_Y;

/// The lowest surface a column reports: one below the search range, for a
/// column with no solid cell in it. Every cell under it is filled, whatever
/// the landform.
pub(crate) const SURFACE_FLOOR_Y: i32 = SURFACE_SEARCH_Y.start - 1;

/// The highest solid density cell of each column, `z * w + x`, within
/// [`SURFACE_SEARCH_Y`]: the surface the terrain fill lays everything at or
/// under, and air or the sea above.
pub(crate) fn surface_heights(
    density: &TerrainDensityGraph,
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
) -> Vec<i32> {
    let (bottom, height) = (SURFACE_SEARCH_Y.start, SURFACE_SEARCH_Y.len());
    let bounds = DensityLatticeBounds::new(x0, bottom, z0, w, height, h);
    master_density_lattice(density, bounds)
        .top_solid_surfaces()
        .into_iter()
        .map(|surf| surf.unwrap_or(SURFACE_FLOOR_Y))
        .collect()
}

fn master_density_lattice(
    density: &TerrainDensityGraph,
    bounds: DensityLatticeBounds,
) -> DensityLattice {
    DensityLattice::sample_channel(
        density.graph(),
        channels::MASTER_DENSITY,
        bounds,
        DensityLatticeCellSize::default(),
    )
    .expect("surface density graph must expose master_density")
}

fn is_ocean_biome(biome: Biome) -> bool {
    spec(biome).flags.ocean
}

fn can_be_beach_base(biome: Biome) -> bool {
    spec(biome).flags.beach_base
}
