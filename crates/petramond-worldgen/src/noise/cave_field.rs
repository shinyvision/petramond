//! Cave sampling, biome queries and bounded excavation composition.
//! Geometry is evaluated before habitat surface treatment and fluid containment.

use super::settings::*;

use super::{cave_density::CaveDensity, cave_walk::WalkField};
use crate::data::underground::{self, LiningFaces, UndergroundBiomes};
use crate::density::terrain::{channels, TerrainDensityGraph, TerrainDensitySpec};
use crate::graph::SamplePoint;
use petramond_world::block::Block;
use petramond_world::chunk::{idx, section_idx, Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_SIZE};
use petramond_world::section::Section;

use super::chamber;

mod aquifer;
mod batch;
mod carve;
mod climate;
mod lattice;
mod point_cache;
mod query_cache;
mod regions;
mod sampling;
mod source;
use carve::BatchCarve;
#[cfg(test)]
use carve::MAX_FLOOR_DEPTH;
#[cfg(test)]
mod separation_tests;
mod territory;
mod volumes;

const LATTICE_STEP: i32 = CAVE_LATTICE_STEP;
const LATTICE_STEP_F: f64 = LATTICE_STEP as f64;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum CaveCut {
    Solid,
    Shell,
    Open,
    Barrier(u16),
    Fill(u16),
}

impl CaveCut {
    #[inline]
    fn is_open(self) -> bool {
        match self {
            Self::Open => true,
            Self::Fill(block) => !Block::from_id(block).is_solid(),
            _ => false,
        }
    }
}

/// Immutable world sources; caches only memoize positional results.
pub struct CaveField {
    seed: u32,
    natural: CaveDensity,
    terrain: TerrainDensityGraph,
    underground: &'static UndergroundBiomes,
    excavations: &'static crate::data::excavations::Excavations,
    chamber_y_span: Option<(i32, i32)>,
    lining_faces: bool,
}

#[derive(Copy, Clone)]
struct Fields {
    carve: bool,
    interior: bool,
    biome: bool,
    excavations: bool,
    positioned: bool,
}

impl Fields {
    const ALL: Fields = Fields {
        carve: true,
        interior: true,
        biome: true,
        excavations: true,
        positioned: true,
    };
}

struct CaveLattice {
    lx0: i32,
    ly0: i32,
    lz0: i32,
    nx: usize,
    ny: usize,
    nz: usize,
    entrance: Vec<f64>,
    density: Vec<f64>,
    noodle_a: Vec<f64>,
    noodle_b: Vec<f64>,
    noodle_toggle: Vec<f64>,
    noodle_width: Vec<f64>,
    climate: [Vec<f64>; 6],
    /// Conservative: no aquifer row can own a cell of this box, so the carve
    /// skips every aquifer read. Exact when `false` is claimed.
    no_aquifer: bool,
    /// Conservative: no row lining any surface can own a cell of this box.
    unlined: bool,
    /// Conservative: only ordinary stone can own a cell of this box, so the
    /// closest-range decision is settled for every cell without asking it.
    plain: bool,
    geology: bool,
    regions: regions::Columns,
    /// Per lattice cell, whether any walk cut reaches it (empty without walks).
    walk_cells: Vec<bool>,
    #[cfg(test)]
    chamber_live: bool,
    fields: Fields,
    walks: Option<WalkField>,
    volumes: volumes::Tiles,
    claims: volumes::claims::Columns,
}

mod lane {
    pub(super) const ENTRANCE: usize = 0;
    pub(super) const INTERIOR: usize = 1;
    pub(super) const NOODLE_TOGGLE: usize = 2;
    pub(super) const NOODLE_A: usize = 3;
    pub(super) const NOODLE_B: usize = 4;
    pub(super) const NOODLE_WIDTH: usize = 5;
    pub(super) const CLIMATE: usize = 6;
    pub(super) const COUNT: usize = 12;
}

/// A vertical cursor reuses horizontal interpolation for every lane.
struct Col<'a> {
    lat: &'a CaveLattice,
    x: i32,
    z: i32,
    region: regions::Column,
    i00: usize,
    i01: usize,
    plane: usize,
    tx: f64,
    tz: f64,
    cell_y: i32,
    cached: u16,
    lo: [f64; lane::COUNT],
    hi: [f64; lane::COUNT],
}

impl CaveField {
    pub fn new(seed: u32) -> Self {
        Self::with_tables(
            seed,
            underground::table(),
            crate::data::excavations::table(),
        )
    }

    pub(super) fn with_tables(
        seed: u32,
        underground: &'static UndergroundBiomes,
        excavations: &'static crate::data::excavations::Excavations,
    ) -> Self {
        Self {
            seed,
            underground,
            chamber_y_span: excavations.y_span,
            excavations,
            lining_faces: underground.lining_faces_vary,
            natural: CaveDensity::new(seed),
            terrain: TerrainDensitySpec::default_surface().build_graph(seed),
        }
    }

    #[cfg(test)]
    pub fn with_table(seed: u32, table: &'static UndergroundBiomes) -> Self {
        Self::with_tables(
            seed,
            table,
            crate::data::excavations::test_table(&[], table),
        )
    }

    /// The addresses of the loaded habitat and excavation catalogs: the
    /// identity every memo of a carve result carries beside the seed.
    pub(crate) fn table_identities(&self) -> [usize; 2] {
        [
            std::ptr::from_ref(self.underground) as usize,
            std::ptr::from_ref(self.excavations) as usize,
        ]
    }

    fn natural_open(&self, [x, y, z]: [i32; 3]) -> bool {
        if y < CAVE_MIN_Y {
            return false;
        }
        let lat = self.build_lattice_filtered(
            x,
            y,
            z,
            x,
            y,
            z,
            Fields {
                carve: true,
                interior: true,
                biome: false,
                excavations: false,
                positioned: false,
            },
        );
        self.cut_unsealed(&mut Col::new(&lat, x, z), y, false, true) == CaveCut::Open
    }

    pub fn cave_carved(&self, x: i32, y: i32, z: i32, surf_y: i32) -> bool {
        if y > surf_y + self.excavations.surface_offset {
            return false;
        }
        let gate = Self::entrance_allowed(y, surf_y);
        let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
        if !gate && !interior && !self.field_at_height(y) {
            return false;
        }
        let lat = self.point_lattice([x, y, z], interior);
        self.cut_from_col(&mut Col::new(&lat, x, z), y, gate, interior)
            .is_open()
    }

    #[inline]
    fn cut_unsealed(&self, c: &mut Col, y: i32, gate: bool, interior: bool) -> CaveCut {
        let mut density = 1.0_f64;
        if gate {
            density = density.min(c.get(lane::ENTRANCE, y));
        }
        if interior {
            density = density.min(c.get(lane::INTERIOR, y));
            if density < 0.0 {
                return CaveCut::Open;
            }
            if y > CAVE_MIN_Y + 4 && c.get(lane::NOODLE_TOGGLE, y) >= 0.0 {
                let noodle = c
                    .get(lane::NOODLE_A, y)
                    .abs()
                    .max(c.get(lane::NOODLE_B, y).abs())
                    * 1.5
                    - c.get(lane::NOODLE_WIDTH, y);
                density = density.min(noodle);
            }
        }
        if density < 0.0 {
            return CaveCut::Open;
        }
        if interior || gate {
            if let Some(walks) = c.lat.walks.as_ref().filter(|_| c.lat.walk_cells[c.cell()]) {
                let walk = walks.at([c.x as f64, y as f64, c.z as f64]);
                let floor = ((y - CAVE_MIN_Y) as f64 / CAVE_FLOOR_FADE).clamp(0.0, 1.0);
                density = density.min(0.12 + (walk - 0.12) * floor);
            }
        }
        if density < 0.0 {
            return CaveCut::Open;
        }
        let max_shell = if c.lat.fields.biome {
            self.underground.bounds
        } else {
            1.0
        };
        if density >= 0.05 * max_shell {
            return CaveCut::Solid;
        }
        let shell_width = if !c.lat.fields.biome {
            1.0
        } else if c.lat.plain {
            self.underground.base
        } else {
            c.region.id(self, y).map_or_else(
                || self.underground.shell_at(c.climate(y), y),
                |id| self.underground.shell(id),
            )
        };
        if density < 0.05 * shell_width {
            CaveCut::Shell
        } else {
            CaveCut::Solid
        }
    }

    #[inline]
    fn cut_col(&self, c: &mut Col, y: i32, surf_y: i32) -> CaveCut {
        if y > surf_y + self.excavations.surface_offset {
            return CaveCut::Solid;
        }
        let gate = Self::entrance_allowed(y, surf_y);
        let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
        if !gate && !interior && !self.field_at_height(y) {
            return CaveCut::Solid;
        }
        self.cut_from_col(c, y, gate, interior)
    }

    /// [`Self::cut_col`] for a walk that has already read the cell's
    /// positioned-field treatment.
    #[inline]
    fn cut_col_treated(
        &self,
        c: &mut Col,
        y: i32,
        surf_y: i32,
        treatment: volumes::Cell,
    ) -> CaveCut {
        if y > surf_y + self.excavations.surface_offset {
            return CaveCut::Solid;
        }
        let gate = Self::entrance_allowed(y, surf_y);
        let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
        if !gate && !interior && !self.field_at_height(y) {
            return CaveCut::Solid;
        }
        self.cut_from_col_treated(c, y, gate, interior, treatment)
    }

    #[cfg(test)]
    fn cut_lat(&self, lat: &CaveLattice, x: i32, y: i32, z: i32, surf_y: i32) -> CaveCut {
        self.cut_col(&mut Col::new(lat, x, z), y, surf_y)
    }

    #[cfg(test)]
    fn carved_lat(&self, lat: &CaveLattice, x: i32, y: i32, z: i32, surf_y: i32) -> bool {
        self.cut_lat(lat, x, y, z, surf_y).is_open()
    }

    #[cfg(test)]
    pub(super) fn chamber_field(&self, lo: [i32; 3], hi: [i32; 3]) -> chamber::ChamberField {
        chamber::ChamberField::gather(
            chamber::CandidateCache::shared(),
            self.underground,
            self.excavations,
            self.seed,
            [lo, hi],
            |x, y, z| self.base_biome_at([x, y, z]),
            |p| self.natural_open(p),
        )
    }

    #[cfg(test)]
    pub(super) fn knead_sample(&self, x: i32, y: i32, z: i32) -> f64 {
        self.natural.knead([x as f64, y as f64, z as f64])
    }

    #[inline]
    fn biome_id_lat(&self, lat: &CaveLattice, x: i32, y: i32, z: i32) -> u8 {
        lat.claims
            .at([x, y, z])
            .or_else(|| lat.regions.at(x, z).id(self, y))
            .unwrap_or_else(|| self.underground.id_at(lat.climate_at(x, y, z), y))
    }

    /// [`Self::biome_id_lat`] for the cursor's own column: the cursor's lerp
    /// sequence is the lattice's, so the id is the same, without re-deriving
    /// the horizontal interpolation of six lanes.
    #[inline]
    fn biome_id_col(&self, c: &mut Col, y: i32) -> u8 {
        c.lat
            .claims
            .at([c.x, y, c.z])
            .or_else(|| c.region.id(self, y))
            .unwrap_or_else(|| self.underground.id_at(c.climate(y), y))
    }

    #[inline]
    pub(crate) fn field_at_height(&self, y: i32) -> bool {
        self.excavations
            .field_y_span
            .is_some_and(|(lo, hi)| (lo..=hi).contains(&y))
    }

    pub(crate) fn field_space_at(&self, pos: [i32; 3]) -> Option<mod_api::TerrainSpace> {
        if !self.field_at_height(pos[1]) {
            return None;
        }
        volumes::space_at(self, pos)
    }

    #[cfg(test)]
    pub fn underground_biome_at(&self, x: i32, y: i32, z: i32) -> u8 {
        let fields = Fields {
            carve: false,
            interior: false,
            biome: true,
            excavations: true,
            positioned: true,
        };
        let lat = self.build_lattice_filtered(x, y, z, x, y, z, fields);
        self.biome_id_lat(&lat, x, y, z)
    }

    /// [`Self::surface_after_caves`] for every column of one 16×16 chunk:
    /// one lattice per source group instead of one per probed cell, with the
    /// same answer, since corner samples are world-anchored.
    pub fn surfaces_after_caves(&self, ox: i32, oz: i32, surf: &[i32]) -> Vec<i32> {
        let mut probe = ColumnProbe::new(self, ox, oz, surf);
        let mut out = Vec::with_capacity(surf.len());
        for z in 0..SECTION_SIZE as i32 {
            for x in 0..SECTION_SIZE as i32 {
                let (wx, wz) = (ox + x, oz + z);
                let surf_y = surf[(z * SECTION_SIZE as i32 + x) as usize];
                if !probe.carved_at_surface(wx, wz, surf_y) {
                    out.push(surf_y);
                    continue;
                }
                let (band, deep) = probe.lattices(surf_y);
                let mut band = Col::new(band, wx, wz);
                let mut deep = deep.map(|lattice| Col::new(lattice, wx, wz));
                let mut y = surf_y;
                while y >= CAVE_MIN_Y && self.carved_with(&mut band, deep.as_mut(), y, surf_y) {
                    y -= 1;
                }
                out.push(y);
            }
        }
        out
    }

    /// [`Self::feature_surface_after_caves`] for every column of one chunk.
    pub fn feature_surfaces_after_caves(&self, ox: i32, oz: i32, surf: &[i32]) -> Vec<i32> {
        let mut probe = ColumnProbe::new(self, ox, oz, surf);
        let mut out = Vec::with_capacity(surf.len());
        for z in 0..SECTION_SIZE as i32 {
            for x in 0..SECTION_SIZE as i32 {
                let surf_y = surf[(z * SECTION_SIZE as i32 + x) as usize];
                out.push(if probe.carved_at_surface(ox + x, oz + z, surf_y) {
                    CAVE_ENTRANCE_MIN_SURFACE_Y
                        .min(surf_y)
                        .min(petramond_world::chunk::SEA_LEVEL)
                } else {
                    surf_y
                });
            }
        }
        out
    }

    /// [`Self::cave_carved`] through cursors on the chunk's two probe
    /// lattices: the surface band for cells the entrance rules govern, the
    /// interior lattice below it. An interior cell always has the interior
    /// cursor, because the probe builds that lattice before any descent.
    #[inline]
    fn carved_with<'c, 'l>(
        &self,
        band: &'c mut Col<'l>,
        deep: Option<&'c mut Col<'l>>,
        y: i32,
        surf_y: i32,
    ) -> bool {
        if y > surf_y + self.excavations.surface_offset {
            return false;
        }
        let gate = Self::entrance_allowed(y, surf_y);
        let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
        if !gate && !interior && !self.field_at_height(y) {
            return false;
        }
        let cursor = if interior {
            deep.expect("interior lattice built before the descent")
        } else {
            band
        };
        self.cut_from_col(cursor, y, gate, interior).is_open()
    }

    pub fn surface_after_caves(&self, x: i32, z: i32, surf_y: i32) -> i32 {
        if !self.cave_carved(x, surf_y, z, surf_y) {
            return surf_y;
        }
        let mut y = surf_y;
        while y >= CAVE_MIN_Y && self.cave_carved(x, y, z, surf_y) {
            y -= 1;
        }
        y
    }

    pub fn feature_surface_after_caves(&self, x: i32, z: i32, surf_y: i32) -> i32 {
        if self.cave_carved(x, surf_y, z, surf_y) {
            CAVE_ENTRANCE_MIN_SURFACE_Y
                .min(surf_y)
                .min(petramond_world::chunk::SEA_LEVEL)
        } else {
            surf_y
        }
    }

    pub fn section_may_carve(cy: i32, surf_min: i32, surf_max: i32) -> bool {
        let y0 = cy * SECTION_SIZE as i32;
        let y1 = y0 + SECTION_SIZE as i32 - 1;
        if y0 > surf_max + crate::data::excavations::table().surface_offset {
            return false;
        }
        if crate::data::excavations::table()
            .field_y_span
            .is_some_and(|(lo, hi)| y0 <= hi && y1 >= lo)
        {
            return true;
        }
        if y1 < CAVE_MIN_Y {
            return false;
        }

        let interior = y0 <= surf_max - CAVE_SURFACE_BUFFER;
        let entrance = surf_max >= CAVE_ENTRANCE_MIN_SURFACE_Y
            && y0 <= surf_max
            && y1 >= surf_min - CAVE_ENTRANCE_MAX_DEPTH;
        interior || entrance
    }

    #[inline]
    fn batch_min_y(&self) -> i32 {
        let natural = if self.underground.lining_floor_under_world_floor {
            CAVE_MIN_Y - self.underground.lining_floor_depth_max.max(1)
        } else {
            CAVE_MIN_Y
        };
        self.excavations
            .rows
            .iter()
            .filter_map(|r| r.field())
            .fold(natural, |y, f| {
                y.min(
                    (f.y[0] - self.underground.lining_floor_depth_max.max(1))
                        .max(petramond_world::chunk::WORLD_MIN_Y + 1),
                )
            })
    }

    #[inline]
    fn batch_y_pad(&self) -> i32 {
        if self.lining_faces {
            self.underground.lining_floor_depth_max.max(1)
        } else {
            0
        }
    }

    #[inline]
    fn batch_y_pad_low(&self, y0: i32) -> i32 {
        (self.lining_faces && y0 > self.batch_min_y()) as i32
    }

    pub fn carve_chunk(&self, chunk: &mut Chunk, surf: &[i32]) {
        debug_assert_eq!(surf.len(), CHUNK_SX * CHUNK_SZ);
        let (ox, oz) = chunk.chunk_origin_world();
        let mut carved = false;

        let y0 = self.batch_min_y().max(0);
        let y1 = surf
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .saturating_add(self.excavations.surface_offset)
            .min(CHUNK_SY as i32 - 1);
        if y0 > y1 {
            return;
        }
        let lat = self.build_lattice(
            ox,
            y0 - self.batch_y_pad_low(y0),
            oz,
            ox + CHUNK_SX as i32 - 1,
            y1 + self.batch_y_pad(),
            oz + CHUNK_SZ as i32 - 1,
        );
        let batch = BatchCarve::new(self, &lat);
        let mut scratch = carve::Scratch::default();
        let blocks = chunk.blocks_slice_mut();

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let surf_y = surf[z * CHUNK_SX + x];
                let y1 = (surf_y + self.excavations.surface_offset).min(CHUNK_SY as i32 - 1);
                if y0 > y1 {
                    continue;
                }
                let slot = |y| idx(x, y as usize, z);
                let (wx, wz) = (ox + x as i32, oz + z as i32);
                carved |= if batch.faces {
                    batch.column::<true, _>(blocks, slot, wx, wz, y0, y1, surf_y, &mut scratch)
                } else {
                    batch.column::<false, _>(blocks, slot, wx, wz, y0, y1, surf_y, &mut scratch)
                };
            }
        }

        if carved {
            chunk.recompute_heightmap();
            chunk.recompute_random_tick_count();
        }
    }

    pub fn carve_section(&self, section: &mut Section, surf: &[i32]) {
        self.carve_section_with_fields(section, surf, true);
    }

    fn carve_section_with_fields(&self, section: &mut Section, surf: &[i32], positioned: bool) {
        debug_assert_eq!(surf.len(), SECTION_SIZE * SECTION_SIZE);
        let (ox, oy, oz) = section.origin_world();

        let y0 = oy.max(self.batch_min_y());
        let y1 = (oy + SECTION_SIZE as i32 - 1)
            .min(surf.iter().copied().max().unwrap_or(i32::MIN) + self.excavations.surface_offset);
        if y0 > y1 {
            return;
        }
        let lat = self.build_lattice_filtered(
            ox,
            y0 - self.batch_y_pad_low(y0),
            oz,
            ox + SECTION_SIZE as i32 - 1,
            y1 + self.batch_y_pad(),
            oz + SECTION_SIZE as i32 - 1,
            Fields {
                positioned,
                ..Fields::ALL
            },
        );
        let batch = BatchCarve::new(self, &lat);
        let mut scratch = carve::Scratch::default();
        section.edit_ids_bulk(|blocks| {
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let surf_y = surf[z * SECTION_SIZE + x];
                    let y1 = (oy + SECTION_SIZE as i32 - 1)
                        .min(surf_y + self.excavations.surface_offset);
                    if y0 > y1 {
                        continue;
                    }
                    let slot = |y| section_idx(x, (y - oy) as usize, z);
                    let (wx, wz) = (ox + x as i32, oz + z as i32);
                    if batch.faces {
                        batch.column::<true, _>(blocks, slot, wx, wz, y0, y1, surf_y, &mut scratch);
                    } else {
                        batch.column::<false, _>(
                            blocks,
                            slot,
                            wx,
                            wz,
                            y0,
                            y1,
                            surf_y,
                            &mut scratch,
                        );
                    }
                }
            }
        });
    }

    #[inline]
    fn entrance_allowed(y: i32, surf_y: i32) -> bool {
        let depth = surf_y - y;
        surf_y >= CAVE_ENTRANCE_MIN_SURFACE_Y && (0..=CAVE_ENTRANCE_MAX_DEPTH).contains(&depth)
    }
}

#[cfg(test)]
mod tests;

/// One chunk's post-cave surface probes: the surface band lattice (built on
/// first use) and the interior lattice (built once a column descends).
struct ColumnProbe<'a> {
    field: &'a CaveField,
    ox: i32,
    oz: i32,
    surf_min: i32,
    surf_max: i32,
    band: Option<CaveLattice>,
    deep: Option<CaveLattice>,
}

impl<'a> ColumnProbe<'a> {
    fn new(field: &'a CaveField, ox: i32, oz: i32, surf: &[i32]) -> Self {
        debug_assert_eq!(surf.len(), SECTION_SIZE * SECTION_SIZE);
        let (surf_min, surf_max) = surf
            .iter()
            .fold((i32::MAX, i32::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)));
        Self {
            field,
            ox,
            oz,
            surf_min,
            surf_max,
            band: None,
            deep: None,
        }
    }

    fn fields(interior: bool) -> Fields {
        Fields {
            carve: true,
            interior,
            biome: false,
            excavations: true,
            positioned: true,
        }
    }

    fn band(&mut self) -> &CaveLattice {
        let (field, ox, oz) = (self.field, self.ox, self.oz);
        let (lo, hi) = (
            self.surf_min - CAVE_SURFACE_BUFFER,
            self.surf_max + field.excavations.surface_offset,
        );
        self.band.get_or_insert_with(|| {
            field.build_lattice_filtered(
                ox,
                lo,
                oz,
                ox + SECTION_SIZE as i32 - 1,
                hi,
                oz + SECTION_SIZE as i32 - 1,
                Self::fields(false),
            )
        })
    }

    /// Whether the column's surface cell is carved: the entrance rules' own
    /// lattice answers it, as the point path's non-interior lattice does.
    fn carved_at_surface(&mut self, wx: i32, wz: i32, surf_y: i32) -> bool {
        let field = self.field;
        let band = self.band();
        let mut cursor = Col::new(band, wx, wz);
        field.carved_with(&mut cursor, None, surf_y, surf_y)
    }

    /// Both lattices a descent from `surf_y` can read.
    fn lattices(&mut self, surf_y: i32) -> (&CaveLattice, Option<&CaveLattice>) {
        let (field, ox, oz, surf_max) = (self.field, self.ox, self.oz, self.surf_max);
        if surf_y - CAVE_SURFACE_BUFFER >= CAVE_MIN_Y {
            self.deep.get_or_insert_with(|| {
                field.build_lattice_filtered(
                    ox,
                    CAVE_MIN_Y,
                    oz,
                    ox + SECTION_SIZE as i32 - 1,
                    surf_max - CAVE_SURFACE_BUFFER,
                    oz + SECTION_SIZE as i32 - 1,
                    Self::fields(true),
                )
            });
        }
        self.band();
        (self.band.as_ref().expect("built above"), self.deep.as_ref())
    }
}
