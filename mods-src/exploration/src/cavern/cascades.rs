//! Cavern cascades. Each lattice cell settles once, through the shared memo.
//!
//! Own pipeline per cell, not the dressing's batches. Rarity roll is free, biome pre-gate costs
//! one tiny crossing, height scan two batches. Band probe and giant-intruder sweep only happen
//! when the floor has a real contour edge.
//!
//! Terrain alone decides. Giants in a basin's water, on it, or breaking its proof get
//! suppressed; the giant pass checks [`giant_suppressed`] by position. Giants never veto basins.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use mod_sdk::*;

use super::claims::Emitter;
use super::giants::{standing_giants_over, Candidate};
use super::CLAIM_ROWS;
use crate::cascade;
use crate::content::Content;
use crate::probe::{self, Deferred, Memo, Settled};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeCell {
    Unknown,
    Queued,
    Solid,
    Air,
    Fluid,
}

struct CellTerrain {
    origin: [i32; 3],
    cells: Vec<ProbeCell>,
    query: probe::Query<TerrainSpace>,
    failed: bool,
}

impl CellTerrain {
    fn new(cell: &cascade::Cell) -> Self {
        Self::with_query(cell, terrain_space_at)
    }

    fn with_query(cell: &cascade::Cell, query: probe::Query<TerrainSpace>) -> Self {
        let side = cascade::LATTICE as usize;
        let height = cascade::LATTICE_Y as usize;
        Self {
            origin: [
                cell.lx * cascade::LATTICE,
                cell.ly * cascade::LATTICE_Y,
                cell.lz * cascade::LATTICE,
            ],
            cells: vec![ProbeCell::Unknown; side * height * side],
            query,
            failed: false,
        }
    }

    fn slot(&self, p: [i32; 3]) -> Option<usize> {
        let x = p[0].wrapping_sub(self.origin[0]) as u32;
        let y = p[1].wrapping_sub(self.origin[1]) as u32;
        let z = p[2].wrapping_sub(self.origin[2]) as u32;
        if x >= cascade::LATTICE as u32
            || y >= cascade::LATTICE_Y as u32
            || z >= cascade::LATTICE as u32
        {
            return None;
        }
        let side = cascade::LATTICE as usize;
        Some(((y as usize * side + x as usize) * side) + z as usize)
    }

    fn ask(&mut self, positions: impl IntoIterator<Item = [i32; 3]>) -> bool {
        if self.failed {
            return false;
        }
        let mut fresh = Vec::new();
        let mut slots = Vec::new();
        for p in positions {
            let Some(slot) = self.slot(p) else {
                self.failed = true;
                return false;
            };
            if self.cells[slot] == ProbeCell::Unknown {
                self.cells[slot] = ProbeCell::Queued;
                fresh.push(p);
                slots.push(slot);
            }
        }
        let Some(reply) = probe::ask(fresh, self.query) else {
            self.failed = true;
            return false;
        };
        for (slot, space) in slots.into_iter().zip(reply) {
            self.cells[slot] = match space {
                TerrainSpace::Solid => ProbeCell::Solid,
                TerrainSpace::Air => ProbeCell::Air,
                TerrainSpace::Fluid => ProbeCell::Fluid,
            };
        }
        true
    }

    fn space(&self, p: [i32; 3]) -> Option<TerrainSpace> {
        let slot = self.slot(p)?;
        match self.cells[slot] {
            ProbeCell::Solid => Some(TerrainSpace::Solid),
            ProbeCell::Air => Some(TerrainSpace::Air),
            ProbeCell::Fluid => Some(TerrainSpace::Fluid),
            _ => None,
        }
    }

    fn solid(&self, p: [i32; 3]) -> Option<bool> {
        self.space(p).map(|space| space == TerrainSpace::Solid)
    }
}

type Key = (u32, i32, i32, i32);
const CAPACITY: usize = 16384;

thread_local! {
    static CACHE: RefCell<Settled<Key, Option<Rc<cascade::Feature>>>> =
        RefCell::new(Settled::new(CAPACITY));
}

pub(super) fn overlapping(
    seed: u32,
    ours: u8,
    origin: [i32; 3],
) -> Result<Vec<Rc<cascade::Feature>>, Deferred> {
    let mut features = Vec::new();
    for cell in cascade::cells_overlapping(origin, CLAIM_ROWS) {
        features.extend(cascade_cell(seed, ours, cell)?);
    }
    Ok(features)
}

pub(super) fn emit(content: &Content, out: &mut Emitter, features: &[Rc<cascade::Feature>]) {
    for f in features {
        for &(cell, kind) in &f.writes {
            let block = match kind {
                cascade::Kind::Water => content.water,
                cascade::Kind::Silt => content.silt,
                cascade::Kind::Air => content.air,
            };
            out.push_over_terrain(cell, block);
        }
        for &cell in &f.reserves {
            out.reserve(cell);
        }
    }
}

pub(super) fn giant_suppressed(seed: u32, ours: u8, c: &Candidate) -> Result<bool, Deferred> {
    let r = c.giant.reach();
    let lo = [c.x - r, c.cell_floor_y, c.z - r];
    let hi = [c.x + r, c.cell_top_y() + c.giant.rise(), c.z + r];
    for cell in cascade::cells_overlapping_box(lo, hi) {
        if let Some(f) = cascade_cell(seed, ours, cell)? {
            if f.suppressed.contains(&c.lat) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn cascade_cell(
    seed: u32,
    ours: u8,
    cell: (i32, i32, i32),
) -> Result<Option<Rc<cascade::Feature>>, Deferred> {
    let key = (seed, cell.0, cell.1, cell.2);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key)) {
        return Ok(hit);
    }
    let out = probe::settle(
        Memo::HOST,
        &cascade::memo_key(ours, cell),
        |bytes| cascade::Feature::decode(bytes).map(|f| f.map(Rc::new)),
        |f: &Option<Rc<cascade::Feature>>| cascade::Feature::encode(f.as_deref()),
        || compute_cascade_cell(seed, ours, cell),
    )?;
    CACHE.with(|c| c.borrow_mut().insert(key, out.clone()));
    Ok(out)
}

/// Uncached path: rarity roll, cheap biome pre-gate, coarse height scan, contour traces, then band
/// probe and basin build with containment flood per trace. Terrain only.
///
/// Giants added after, adapt to what's there. Stay clear of water or get suppressed, never override
/// a basin. Reads are positional so any worker gets the same answer.
fn compute_cascade_cell(
    seed: u32,
    ours: u8,
    cell: (i32, i32, i32),
) -> Option<Rc<cascade::Feature>> {
    let (lx, ly, lz) = cell;
    let c = cascade::Cell::roll(seed, lx, ly, lz)?;
    let biomes = probe::ask(c.gate_points(), underground_biome_at)?;
    if !biomes.contains(&ours) {
        return None;
    }
    let traces = site_traces(&c)?;
    if traces.is_empty() {
        return None;
    }
    let anchors: Vec<[i32; 3]> = traces
        .iter()
        .map(|t| [t.anchor.0, t.s0, t.anchor.1])
        .collect();
    let anchor_biomes = probe::ask(anchors, underground_biome_at)?;
    let mut reads = CellTerrain::new(&c);
    for (t, &b) in traces.iter().zip(&anchor_biomes) {
        if b != ours {
            continue;
        }
        let mut plan: Vec<[i32; 3]> = Vec::new();
        t.plan(|p| plan.push(p));
        if !reads.ask(plan.iter().copied()) {
            return None;
        }
        if plan
            .iter()
            .any(|&p| reads.space(p) == Some(TerrainSpace::Fluid))
        {
            continue;
        }
        let terrain = |p: [i32; 3]| reads.solid(p);
        let Ok(built) = t.build(&terrain) else {
            continue;
        };
        let intruders = intruders_over(seed, ours, &built);
        return Some(Rc::new(built.finish(&terrain, &intruders)));
    }
    None
}

fn site_traces(c: &cascade::Cell) -> Option<Vec<cascade::Trace>> {
    let mut coarse: Vec<[i32; 3]> = Vec::new();
    c.coarse_plan(|p| coarse.push(p));
    let space = probe::ask(coarse, terrain_space_at)?;
    let rock: Vec<bool> = space.iter().map(|s| *s == TerrainSpace::Solid).collect();
    let free: Vec<bool> = space.iter().map(|s| *s == TerrainSpace::Air).collect();
    Some(c.traces(&rock, &free))
}

fn intruders_over(seed: u32, ours: u8, built: &cascade::Built) -> Vec<cascade::Intruder> {
    standing_giants_over(seed, ours, built.domain_lo, built.domain_hi)
        .into_iter()
        .map(|(g, root)| {
            let mut solid = BTreeSet::new();
            g.giant.emit(|dx, dy, dz, _| {
                solid.insert([root[0] + dx, root[1] + dy, root[2] + dz]);
            });
            cascade::Intruder {
                key: g.lat,
                root,
                solid,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    thread_local! {
        static REQUESTS: RefCell<Vec<Vec<[i32; 3]>>> = const { RefCell::new(Vec::new()) };
    }

    fn terrain(positions: Vec<[i32; 3]>) -> Vec<TerrainSpace> {
        REQUESTS.with(|requests| requests.borrow_mut().push(positions.clone()));
        positions
            .iter()
            .map(|p| match p[0] {
                -190 => TerrainSpace::Solid,
                -188 => TerrainSpace::Fluid,
                _ => TerrainSpace::Air,
            })
            .collect()
    }

    #[test]
    fn cascade_reads_reuse_overlapping_cells_without_shifting_answers() {
        REQUESTS.with(|requests| requests.borrow_mut().clear());
        let cell = cascade::Cell {
            lx: -2,
            ly: -1,
            lz: 3,
        };
        let mut reads = CellTerrain::with_query(&cell, terrain);
        let rock = [-190, -31, 290];
        let air = [-189, -31, 290];
        let fluid = [-188, -31, 291];
        assert!(reads.ask([rock, air, rock]));
        assert!(reads.ask([air, fluid]));
        assert_eq!(
            REQUESTS.with(|requests| requests.borrow().clone()),
            vec![vec![rock, air], vec![fluid]]
        );
        assert_eq!(reads.solid(rock), Some(true));
        assert_eq!(reads.space(air), Some(TerrainSpace::Air));
        assert_eq!(reads.space(fluid), Some(TerrainSpace::Fluid));
        assert_eq!(reads.space([0, 0, 0]), None);
    }
}
