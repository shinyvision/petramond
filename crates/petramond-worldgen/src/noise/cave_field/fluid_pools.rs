//! Fluid pools: the fluid a cave's own hollows hold, one `fluid_pools` row
//! at a time.
//!
//! A pool is the set of open cells CONNECTED to one floor cell by a path that
//! never rises above one surface level. That set is closed sideways and
//! downward by construction, so the rock the cave carved holds the fluid and
//! nothing is added to seal it; two pools of a row that overlap are nested.
//!
//! The level is chosen by raising it until the fill runs past a row's reach,
//! sinks past its floor or outgrows its budget — the first level that went
//! over the hollow's brim — and keeping the last level that fit.
//!
//! A pool is a pure function of (seed, row, lattice cell) and the cave model,
//! so every box derives the same cells; derivation is memoized process-wide.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use super::*;
use crate::data::underground::{FluidPool, POOL_CELL};
use crate::density::surface::SURFACE_FLOOR_Y;
use crate::rng::FeatureRng;

pub(super) type PoolKey = (crate::cache::GenContext, u16, [i32; 3]);

pub(super) type TileKey = (crate::cache::GenContext, [i32; 3]);

pub(super) struct Pool {
    fluid: u16,
    lo: [i32; 3],
    span: [i32; 3],
    cells: Box<[u64]>,
}

impl Pool {
    #[inline]
    fn holds(&self, pos: [i32; 3]) -> bool {
        let d: [i32; 3] = std::array::from_fn(|a| pos[a] - self.lo[a]);
        if (0..3).any(|a| d[a] < 0 || d[a] >= self.span[a]) {
            return false;
        }
        let i = ((d[1] * self.span[2] + d[2]) * self.span[0] + d[0]) as usize;
        self.cells[i / 64] & (1 << (i % 64)) != 0
    }

    #[inline]
    fn reaches(&self, lo: [i32; 3], hi: [i32; 3]) -> bool {
        (0..3).all(|a| self.lo[a] <= hi[a] && self.lo[a] + self.span[a] > lo[a])
    }
}

#[derive(Default)]
pub(super) struct Pools {
    near: Vec<Arc<Pool>>,
    index: ColumnIndex,
}

impl Pools {
    pub(super) fn around(field: &CaveField, lo: [i32; 3], hi: [i32; 3]) -> Self {
        Self::gather(field, lo.map(|v| v - 1), hi.map(|v| v + 1))
    }

    pub(super) fn gather(field: &CaveField, lo: [i32; 3], hi: [i32; 3]) -> Self {
        Self::gather_rows(field, field.underground.pools.len(), lo, hi)
    }

    fn gather_rows(field: &CaveField, rows: usize, lo: [i32; 3], hi: [i32; 3]) -> Self {
        let mut near = Vec::new();
        for (row, def) in field.underground.pools[..rows].iter().enumerate() {
            let (up, down) = (def.reach_up(), def.reach_down());
            if hi[1] < CAVE_MIN_Y - down || lo[1] > def.max_y {
                continue;
            }
            let cell = |v: i32| v.div_euclid(POOL_CELL);
            let g0 = [
                cell(lo[0] - up[0]),
                cell(lo[1] - up[1]).max(cell(CAVE_MIN_Y)),
                cell(lo[2] - up[2]),
            ];
            let g1 = [
                cell(hi[0] + up[0]),
                cell(hi[1] + down).min(cell(def.max_y)),
                cell(hi[2] + up[2]),
            ];
            for gy in g0[1]..=g1[1] {
                for gz in g0[2]..=g1[2] {
                    for gx in g0[0]..=g1[0] {
                        if let Some(pool) = pool_at(field, row, [gx, gy, gz]) {
                            if pool.reaches(lo, hi) {
                                near.push(pool);
                            }
                        }
                    }
                }
            }
        }
        let index = ColumnIndex::build(&near, lo, hi);
        Self { near, index }
    }

    #[inline]
    pub(super) fn fluid_at(&self, pos: [i32; 3]) -> u16 {
        let air = Block::Air.id();
        if self.near.is_empty() {
            return air;
        }
        let holding = match self.index.at(pos) {
            Some(refs) => refs
                .iter()
                .map(|&k| &self.near[k as usize])
                .find(|pool| pool.holds(pos)),
            None => self.near.iter().find(|pool| pool.holds(pos)),
        };
        holding.map_or(air, |pool| pool.fluid)
    }
}

#[derive(Default)]
struct ColumnIndex {
    lo: [i32; 2],
    dims: [usize; 2],
    starts: Box<[u32]>,
    refs: Box<[u32]>,
}

impl ColumnIndex {
    const COLUMN: i32 = 8;

    fn build(near: &[Arc<Pool>], lo: [i32; 3], hi: [i32; 3]) -> Self {
        if near.is_empty() {
            return Self::default();
        }
        let lo = [lo[0], lo[2]];
        let dims = [
            ((hi[0] - lo[0]) / Self::COLUMN + 1) as usize,
            ((hi[2] - lo[1]) / Self::COLUMN + 1) as usize,
        ];
        let covered = |pool: &Pool| -> Vec<usize> {
            let range = |axis: usize, lane: usize| {
                let first = (pool.lo[axis] - lo[lane]).max(0) / Self::COLUMN;
                let last = (pool.lo[axis] + pool.span[axis] - 1 - lo[lane]) / Self::COLUMN;
                first as usize..=(last as usize).min(dims[lane] - 1)
            };
            let xs = range(0, 0);
            range(2, 1)
                .flat_map(|cz| xs.clone().map(move |cx| cz * dims[0] + cx))
                .collect()
        };
        let columns: Vec<Vec<usize>> = near.iter().map(|pool| covered(pool)).collect();
        let mut starts = vec![0u32; dims[0] * dims[1] + 1];
        for column in columns.iter().flatten() {
            starts[column + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        let mut fill = starts.clone();
        let mut refs = vec![0u32; starts[starts.len() - 1] as usize];
        for (k, covered) in columns.iter().enumerate() {
            for &column in covered {
                refs[fill[column] as usize] = k as u32;
                fill[column] += 1;
            }
        }
        Self {
            lo,
            dims,
            starts: starts.into(),
            refs: refs.into(),
        }
    }

    #[inline]
    fn at(&self, pos: [i32; 3]) -> Option<&[u32]> {
        let (dx, dz) = (pos[0] - self.lo[0], pos[2] - self.lo[1]);
        if dx < 0 || dz < 0 {
            return None;
        }
        let (cx, cz) = ((dx / Self::COLUMN) as usize, (dz / Self::COLUMN) as usize);
        if cx >= self.dims[0] || cz >= self.dims[1] {
            return None;
        }
        let column = cz * self.dims[0] + cx;
        Some(&self.refs[self.starts[column] as usize..self.starts[column + 1] as usize])
    }
}

fn pool_at(field: &CaveField, row: usize, g: [i32; 3]) -> Option<Arc<Pool>> {
    let def = &field.underground.pools[row];
    let mut rng = FeatureRng::positional(field.seed, def.salt, g[0], g[1], g[2]);
    let height = (g[1] * POOL_CELL + POOL_CELL / 2 - def.anchor_y).max(0) as f32;
    if !rng.chance(def.chance * petramond_math::detmath::expf(-height / def.height_scale)) {
        return None;
    }
    let key = (field.context(), row as u16, g);
    field
        .memos()
        .pools
        .get_or_insert(key, move || derive(field, row, g, rng))
}

fn derive(field: &CaveField, row: usize, g: [i32; 3], mut rng: FeatureRng) -> Option<Arc<Pool>> {
    let def = &field.underground.pools[row];
    let origin: [i32; 3] = g.map(|v| v * POOL_CELL);
    let ceiling = roof(field, origin, POOL_CELL + def.reach, def.surface_clearance).min(def.max_y);
    let reach = POOL_CELL + def.reach;
    let claimed = Pools::gather_rows(
        field,
        row,
        [
            origin[0] - reach,
            origin[1] - def.max_drop - def.max_sink,
            origin[2] - reach,
        ],
        [origin[0] + reach, ceiling, origin[2] + reach],
    );
    let mut tiles = Tiles::default();
    let floor = floor_in_cell(field, def, &claimed, &mut tiles, origin, ceiling, &mut rng)?;
    let lo = [
        floor[0] - def.reach,
        floor[1] - def.max_sink,
        floor[2] - def.reach,
    ];
    let hi = [
        floor[0] + def.reach,
        (floor[1] + def.max_depth).min(ceiling),
        floor[2] + def.reach,
    ];
    if aquifer_possible(field, lo, hi) {
        return None;
    }
    flood(field, def, &claimed, &mut tiles, floor, lo, hi).map(Arc::new)
}

fn aquifer_possible(field: &CaveField, lo: [i32; 3], hi: [i32; 3]) -> bool {
    field.underground.aquifer_y_span.is_some()
        && field
            .underground_biome_ids_in_box(lo.map(|v| v - 1), hi.map(|v| v + 1))
            .ids()
            .iter()
            .any(|&id| field.underground.aquifer(id).is_some())
}

fn roof(field: &CaveField, at: [i32; 3], reach: i32, clearance: i32) -> i32 {
    let mut base = i32::MAX;
    for z in (at[2] - reach..=at[2] + reach).step_by(POOL_CELL as usize) {
        for x in (at[0] - reach..=at[0] + reach).step_by(POOL_CELL as usize) {
            base = base.min(field.climate_column(x, z)[5] as i32);
        }
    }
    base - clearance
}

const FLOOD_FIELDS: Fields = Fields {
    carve: true,
    interior: true,
    biome: false,
    excavations: true,
    positioned: true,
    fluids: false,
};

fn floor_in_cell(
    field: &CaveField,
    def: &FluidPool,
    claimed: &Pools,
    tiles: &mut Tiles,
    origin: [i32; 3],
    ceiling: i32,
    rng: &mut FeatureRng,
) -> Option<[i32; 3]> {
    let lo = origin[1].max(CAVE_MIN_Y);
    let hi = (origin[1] + POOL_CELL - 1).min(ceiling);
    if lo > hi {
        return None;
    }
    let mut open = 0;
    let mut chosen = None;
    for dz in (0..POOL_CELL).step_by(LATTICE_STEP as usize) {
        for dx in (0..POOL_CELL).step_by(LATTICE_STEP as usize) {
            let (x, z) = (origin[0] + dx, origin[2] + dz);
            let mut cursor = Col::new(tiles.lattice(field, origin), x, z);
            for y in (lo..=hi).step_by(LATTICE_STEP as usize) {
                if probe(field, claimed, &mut cursor, y) == Probe::Open {
                    open += 1;
                    if rng.next_i32(1, open) == 1 {
                        chosen = Some([x, y, z]);
                    }
                }
            }
        }
    }
    let mut at = chosen?;
    for _ in 0..def.max_drop {
        let below = [at[0], at[1] - 1, at[2]];
        match tiles.probe(field, claimed, below) {
            Probe::Open => at = below,
            Probe::Rock => return Some(at),
            Probe::Blocked => return None,
        }
    }
    None
}

#[derive(Default)]
struct Tiles {
    last: Option<([i32; 3], Arc<CaveLattice>)>,
}

impl Tiles {
    fn lattice(&mut self, field: &CaveField, pos: [i32; 3]) -> &CaveLattice {
        let tile = pos.map(|v| v.div_euclid(POOL_CELL));
        if !matches!(&self.last, Some((at, _)) if *at == tile) {
            let key = (field.context(), tile);
            let lattice = field.memos().pool_tiles.get_or_insert(key, || {
                let lo = tile.map(|v| v * POOL_CELL);
                let hi = lo.map(|v| v + POOL_CELL - 1);
                Arc::new(field.build_lattice_filtered(
                    lo[0],
                    lo[1],
                    lo[2],
                    hi[0],
                    hi[1],
                    hi[2],
                    FLOOD_FIELDS,
                ))
            });
            self.last = Some((tile, lattice));
        }
        &self.last.as_ref().expect("tile just cached").1
    }

    fn probe(&mut self, field: &CaveField, claimed: &Pools, pos: [i32; 3]) -> Probe {
        if pos[1] < CAVE_MIN_Y {
            return Probe::Rock;
        }
        let lattice = self.lattice(field, pos);
        probe(
            field,
            claimed,
            &mut Col::new(lattice, pos[0], pos[2]),
            pos[1],
        )
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Probe {
    Open,
    Rock,
    Blocked,
}

fn probe(field: &CaveField, claimed: &Pools, c: &mut Col, y: i32) -> Probe {
    if y < CAVE_MIN_Y {
        return Probe::Rock;
    }
    if y > SURFACE_FLOOR_Y && y > field.density_surface(c.x, c.z) {
        return Probe::Blocked;
    }
    if !matches!(c.lat.volumes.at([c.x, y, c.z]), volumes::Cell::Untouched) {
        return Probe::Blocked;
    }
    if !field.cut_unsealed(c, y, true, true).is_air() {
        return Probe::Rock;
    }
    if claimed.fluid_at([c.x, y, c.z]) != Block::Air.id() {
        return Probe::Blocked;
    }
    Probe::Open
}

fn flood(
    field: &CaveField,
    def: &FluidPool,
    claimed: &Pools,
    tiles: &mut Tiles,
    floor: [i32; 3],
    lo: [i32; 3],
    hi: [i32; 3],
) -> Option<Pool> {
    let span: [i32; 3] = std::array::from_fn(|a| hi[a] - lo[a] + 1);
    let index =
        |p: [i32; 3]| (((p[1] - lo[1]) * span[2] + p[2] - lo[2]) * span[0] + p[0] - lo[0]) as usize;
    let mut seen = vec![false; (span[0] * span[1] * span[2]) as usize];
    let mut queue = BinaryHeap::new();
    let mut taken: Vec<([i32; 3], i32)> = Vec::new();
    let mut limit = hi[1];
    let mut level = floor[1];
    seen[index(floor)] = true;
    queue.push(Reverse((floor[1], floor)));
    while let Some(Reverse((_, cell))) = queue.pop() {
        level = level.max(cell[1]);
        if level > limit {
            break;
        }
        taken.push((cell, level));
        if taken.len() > def.budget {
            limit = level - 1;
            break;
        }
        for step in [
            [-1, 0, 0],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let next: [i32; 3] = std::array::from_fn(|a| cell[a] + step[a]);
            let escapes = (0..3).any(|a| next[a] < lo[a] || next[a] > hi[a])
                || match tiles.probe(field, claimed, next) {
                    Probe::Rock => continue,
                    Probe::Blocked => true,
                    Probe::Open => false,
                };
            if escapes {
                limit = limit.min(level.max(next[1]) - 1);
                continue;
            }
            if !std::mem::replace(&mut seen[index(next)], true) {
                queue.push(Reverse((next[1], next)));
            }
        }
        if level > limit {
            break;
        }
    }
    let level = limit.min(level);
    if level < floor[1] {
        return None;
    }
    build(
        def.fluid,
        taken.iter().filter(|(_, at)| *at <= level).map(|(c, _)| *c),
    )
}

fn build(fluid: u16, cells: impl Iterator<Item = [i32; 3]> + Clone) -> Option<Pool> {
    let mut lo = [i32::MAX; 3];
    let mut hi = [i32::MIN; 3];
    for cell in cells.clone() {
        for axis in 0..3 {
            lo[axis] = lo[axis].min(cell[axis]);
            hi[axis] = hi[axis].max(cell[axis]);
        }
    }
    if lo[0] > hi[0] {
        return None;
    }
    let span: [i32; 3] = std::array::from_fn(|a| hi[a] - lo[a] + 1);
    let mut bits = vec![0u64; ((span[0] * span[1] * span[2]) as usize).div_ceil(64)];
    for cell in cells {
        let d: [i32; 3] = std::array::from_fn(|a| cell[a] - lo[a]);
        let i = ((d[1] * span[2] + d[2]) * span[0] + d[0]) as usize;
        bits[i / 64] |= 1 << (i % 64);
    }
    Some(Pool {
        fluid,
        lo,
        span,
        cells: bits.into_boxed_slice(),
    })
}

#[cfg(test)]
mod tests;
