use super::*;
use crate::data::excavations::effects::{Course, MaterialFilter};
use crate::data::excavations::Bounds;
use crate::data::excavations::{Excavation, FieldShape};
use crate::formula::Inputs;
use crate::rng::FeatureRng;
use std::sync::Arc;

pub(super) mod claims;
mod tile;
use tile::build_tile;
pub(in crate::noise::cave_field) use tile::{AnchorKey, ChunkAnchors, PlaneKey};

pub(super) type SiteKey = (crate::cache::GenContext, u64, [i32; 2]);
pub(super) type TileKey = (crate::cache::GenContext, [i32; 3]);

#[derive(Clone, Copy)]
pub(super) struct Site {
    center: [i32; 3],
    radius: i32,
    height: i32,
    bounds: Bounds,
}

impl Site {
    fn inputs(self, [x, y, z]: [i32; 3], surface: i32) -> Inputs {
        Inputs([
            x as f64,
            y as f64,
            z as f64,
            self.center[0] as f64,
            self.center[1] as f64,
            self.center[2] as f64,
            self.radius as f64,
            self.height as f64,
            surface as f64,
            petramond_world::chunk::SEA_LEVEL as f64,
        ])
    }
}

#[derive(Clone, Copy, Default)]
pub(super) enum Cell {
    #[default]
    Untouched,
    Surface,
    Fill(Fill),
}

#[derive(Clone, Copy)]
pub(super) struct Fill {
    pub block: u16,
    pub biome: u8,
    pub replace: MaterialFilter,
}

#[derive(Clone, Copy)]
pub(super) struct Seal {
    key: u16,
    pub block: u16,
    pub replace: MaterialFilter,
}

#[derive(Clone, Copy)]
pub(super) struct CourseCell {
    key: u16,
    pub block: u16,
    pub anchor_dy: i8,
    pub rule: &'static Course,
}

impl CourseCell {
    pub(super) fn y(&self, tile_y: i32) -> i32 {
        tile_y * 16 + i32::from(self.key % 16)
    }
}

pub(super) struct Tile {
    cells: Option<Box<[Cell]>>,
    touched: u64,
    seals: Box<[Seal]>,
    courses: Box<[CourseCell]>,
}

fn column_key(x: i32, y: i32, z: i32) -> u16 {
    ((z.rem_euclid(16) * 16 + x.rem_euclid(16)) * 16 + y.rem_euclid(16)) as u16
}

#[derive(Default)]
pub(super) struct Tiles {
    origin: [i32; 3],
    nx: usize,
    nz: usize,
    tiles: Vec<Arc<Tile>>,
}

impl Tiles {
    pub(super) fn gather(field: &CaveField, lo: [i32; 3], hi: [i32; 3]) -> Self {
        if field.excavations.field_y_span.is_none() {
            return Self::default();
        }
        let origin = lo.map(|v| v.div_euclid(16));
        let end = hi.map(|v| v.div_euclid(16));
        let nx = (end[0] - origin[0] + 1) as usize;
        let nz = (end[2] - origin[2] + 1) as usize;
        let mut tiles = Vec::new();
        let mut any = false;
        for y in origin[1]..=end[1] {
            for z in origin[2]..=end[2] {
                for x in origin[0]..=end[0] {
                    let pos = [x, y, z];
                    let tile = field
                        .memos()
                        .volume_tiles
                        .get_or_insert((field.context(), pos), || Arc::new(build_tile(field, pos)));
                    any |= tile.cells.is_some();
                    tiles.push(tile);
                }
            }
        }
        if !any {
            return Self::default();
        }
        Self {
            origin,
            nx,
            nz,
            tiles,
        }
    }

    #[inline]
    pub(super) fn touched(&self, lattice: [i32; 3]) -> bool {
        if self.tiles.is_empty() {
            return false;
        }
        let p = lattice.map(|v| v.div_euclid(4));
        let delta: [i32; 3] = std::array::from_fn(|a| p[a] - self.origin[a]);
        if delta.iter().any(|&v| v < 0)
            || delta[0] as usize >= self.nx
            || delta[2] as usize >= self.nz
        {
            return false;
        }
        let i = (delta[1] as usize * self.nz + delta[2] as usize) * self.nx + delta[0] as usize;
        let Some(tile) = self.tiles.get(i).filter(|t| t.cells.is_some()) else {
            return false;
        };
        let sub = lattice.map(|v| v.rem_euclid(4) as u64);
        tile.touched & (1 << ((sub[1] * 4 + sub[2]) * 4 + sub[0])) != 0
    }

    #[inline]
    fn tile(&self, [x, y, z]: [i32; 3]) -> Option<&Tile> {
        if self.tiles.is_empty() {
            return None;
        }
        let p = [x, y, z].map(|v| v.div_euclid(16));
        let delta: [i32; 3] = std::array::from_fn(|a| p[a] - self.origin[a]);
        if delta.iter().any(|&v| v < 0)
            || delta[0] as usize >= self.nx
            || delta[2] as usize >= self.nz
        {
            return None;
        }
        let i = (delta[1] as usize * self.nz + delta[2] as usize) * self.nx + delta[0] as usize;
        self.tiles.get(i).map(Arc::as_ref)
    }

    #[inline]
    pub(super) fn at(&self, [x, y, z]: [i32; 3]) -> Cell {
        self.tile([x, y, z])
            .and_then(|t| t.cells.as_ref())
            .map_or(Cell::Untouched, |cells| {
                cells[(y.rem_euclid(16) as usize * 16 + z.rem_euclid(16) as usize) * 16
                    + x.rem_euclid(16) as usize]
            })
    }

    #[inline]
    pub(super) fn seal_at(&self, [x, y, z]: [i32; 3]) -> Option<Seal> {
        let tile = self.tile([x, y, z])?;
        let key = column_key(x, y, z);
        tile.seals
            .binary_search_by_key(&key, |s| s.key)
            .ok()
            .map(|i| tile.seals[i])
    }

    pub(super) fn column_courses(&self, x: i32, y: i32, z: i32) -> &[CourseCell] {
        let Some(tile) = self.tile([x, y, z]) else {
            return &[];
        };
        let lo = column_key(x, 0, z);
        let start = tile.courses.partition_point(|c| c.key < lo);
        let end = tile.courses.partition_point(|c| c.key < lo + 16);
        &tile.courses[start..end]
    }
}

fn site(field: &CaveField, row: &Excavation, profile: &FieldShape, cell: [i32; 2]) -> Option<Site> {
    let key = (field.context(), row.salt, cell);
    field.memos().volume_sites.get_or_insert(key, || {
        let p = &row.placement;
        let mut rng = FeatureRng::positional(field.seed, row.salt, cell[0], 0, cell[1]);
        if rng.next_i32(0, p.one_in - 1) != 0 {
            return None;
        }
        let choices = (p.spacing - profile.separation) / profile.grid_step;
        let center = [
            cell[0] * p.spacing + rng.next_i32(0, choices - 1) * profile.grid_step,
            rng.next_i32(p.y.0, p.y.1),
            cell[1] * p.spacing + rng.next_i32(0, choices - 1) * profile.grid_step,
        ];
        let belongs = |pos: [i32; 3]| {
            p.underground_biome
                .is_none_or(|biome| field.base_biome_at(pos) == biome)
        };
        if !belongs(std::array::from_fn(|a| {
            center[a] + profile.admission_offset[a]
        })) {
            return None;
        }
        let mut radius = rng.next_i32(profile.radius[0], profile.radius[1]);
        let mut height = rng.next_i32(profile.height[0], profile.height[1]);
        if let Some(probe) = &profile.extent_probe {
            let extent = |start: [i32; 3], axis: usize, sign: i32, limit: i32| {
                let mut distance = 0;
                while distance < limit {
                    let mut pos = start;
                    pos[axis] += distance * sign;
                    if !belongs(pos) {
                        break;
                    }
                    distance += probe.step;
                }
                distance.min(limit)
            };
            let down = extent(center, 1, -1, height) + probe.vertical_expand[0];
            let up = extent(center, 1, 1, height) + probe.vertical_expand[1];
            let ground = [center[0], center[1] - down + 2, center[2]];
            radius = (extent(ground, 0, -1, radius)
                + extent(ground, 0, 1, radius)
                + extent(ground, 2, -1, radius)
                + extent(ground, 2, 1, radius))
                / 4
                + probe.horizontal_expand;
            height = (up + down) / 2;
        }
        if radius <= 0 || height <= 0 {
            return None;
        }
        let mut site = Site {
            center,
            radius,
            height,
            bounds: Bounds {
                min: [
                    center[0] - profile.bound_radius,
                    profile.y[0],
                    center[2] - profile.bound_radius,
                ],
                max: [
                    center[0] + profile.bound_radius,
                    profile.y[1],
                    center[2] + profile.bound_radius,
                ],
            },
        };
        if let Some(formula) = &profile.bounds {
            site.bounds =
                site.bounds
                    .intersect_formula(formula, field.seed, site.inputs([0; 3], 0))?;
        }
        Some(site)
    })
}

pub(super) fn space_at(field: &CaveField, pos: [i32; 3]) -> Option<mod_api::TerrainSpace> {
    let cell = pos.map(|v| v.div_euclid(16));
    let tile = field
        .memos()
        .volume_tiles
        .get_or_insert((field.context(), cell), || {
            Arc::new(build_tile(field, cell))
        });
    let cells = tile.cells.as_ref()?;
    let p = pos.map(|v| v.rem_euclid(16) as usize);
    match cells[(p[1] * 16 + p[2]) * 16 + p[0]] {
        Cell::Fill(Fill { block, .. }) => Some(if block == Block::Air.id() {
            mod_api::TerrainSpace::Air
        } else if Block::from_id(block).is_solid() {
            mod_api::TerrainSpace::Solid
        } else {
            mod_api::TerrainSpace::Fluid
        }),
        _ => None,
    }
}
