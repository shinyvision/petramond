//! Cascades in the cavern: each lattice cell's outcome settled once through
//! the shared memo, the host crossings its pipeline pays, the giant
//! suppression verdict, and emission.
//!
//! A cascade runs its own crossings on its own memoized per-cell pipeline,
//! never riding the dressing's batches: rarity roll (free), biome pre-gate
//! (one tiny crossing), coarse height scan (two batches), and only for a cell
//! whose floor offers a real contour edge the band probe and the
//! giant-intruder sweep. Cascades resolve from the terrain alone and are
//! AUTHORITATIVE: no giant stands in a basin's water, shallow or deep — one in
//! it, on it, or breaking its proof is suppressed — and the giant pass queries
//! that decision positionally ([`giant_suppressed`]). A giant never vetoes a
//! basin, and the ordering is real in every section because it is a
//! positional query, not an emission-order accident.

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

/// A cascade's terrain probes stay inside its own lattice cell. A flat grid
/// makes both overlapping trace reads and the basin's many point lookups cheap.
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

/// `(seed, cell x, y, z)`.
type Key = (u32, i32, i32, i32);
/// Settled cells kept per worker; eviction changes cost, never content.
const CAPACITY: usize = 16384;

thread_local! {
    static CACHE: RefCell<Settled<Key, Option<Rc<cascade::Feature>>>> =
        RefCell::new(Settled::new(CAPACITY));
}

/// Every cascade overlapping the section at `origin` (plus its claim rows).
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

/// Write the cascades — AFTER the giants and BEFORE the dressing. The write
/// order is claims plumbing, not precedence: the basin already decided which
/// giants STAND clear of its water and which are suppressed, and its
/// containment flood modelled exactly the standing bodies as solid — so
/// letting a standing giant's cells win the cell conflicts here is what keeps
/// the world equal to the proof. Before the dressing because the reserves are
/// what keep a flower off the water and a vine out of a fall.
pub(super) fn emit(content: &Content, out: &mut Emitter, features: &[Rc<cascade::Feature>]) {
    for f in features {
        for &(cell, kind) in &f.writes {
            let block = match kind {
                cascade::Kind::Water => content.water,
                // Silt: pool beds, rimstone dams and their foundations — the
                // solid this feature PLACES, which is half of how it seals a
                // rim the terrain left open.
                cascade::Kind::Silt => content.silt,
                // The spill notch and plunge shaft, cut through a natural lip
                // so each pool pours into the next. The one place the pack
                // writes air, and it is a slot for water, not a room: the
                // containment flood proved the result before anything was
                // written.
                cascade::Kind::Air => content.air,
            };
            out.push_over_terrain(cell, block);
        }
        for &cell in &f.reserves {
            out.reserve(cell);
        }
    }
}

/// Does a cascade suppress this giant? Cascades are TERRAIN: the basin's
/// containment proof decides which giants stand, and a giant the proof cannot
/// hold with is skipped — in every section, because the answer is a pure
/// function of `(seed, cell)` through the same memo the emitter uses.
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

/// One lattice cell's cascade outcome, computed once per worker and MEMOIZED.
///
/// The cache is the answer to the purity tax: a cell overlaps dozens of
/// section dispatches, and every one must know the identical outcome, but
/// deriving it costs a site probe and — for the rare survivor — a domain
/// probe and a containment flood. Every worker's instance derives the same
/// outcome, so the first one to claim a cell settles it for the rest; a
/// section arriving while that is under way is dispatched again once the
/// cell is published.
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

/// The uncached pipeline: rarity roll, cheap biome pre-gate, coarse height
/// scan, contour traces, then per trace a band probe and the basin build with
/// its containment flood — all from the TERRAIN ALONE. Only then are the
/// giants folded in, and they ADAPT: kept clear of the water or suppressed,
/// never a veto of the basin. Every read is positional, so every worker that
/// computes this arrives at the same answer.
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
    // The anchors' own biome gate, one small crossing for all tries: a basin
    // heads IN the mushroom cavern, not in whatever cave abuts it.
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
        // The containment proof models rock and room only, so a basin
        // meeting a fluid is not sited.
        if plan
            .iter()
            .any(|&p| reads.space(p) == Some(TerrainSpace::Fluid))
        {
            continue;
        }
        let terrain = |p: [i32; 3]| reads.solid(p);
        // The cell mutex: the FIRST trace that builds owns the cell — a
        // second accepted trace could overlap the first's footprint, which
        // confinement exists to forbid. Rejection is the exception, not the
        // siting strategy; the pondaudit tooling reports the reasons when
        // they are wanted, so nothing is logged here in normal play.
        let Ok(built) = t.build(&terrain) else {
            continue;
        };
        let intruders = intruders_over(seed, ours, &built);
        return Some(Rc::new(built.finish(&terrain, &intruders)));
    }
    None
}

/// The coarse height scan of a rolled cell, read into its contour traces.
/// `None` when the host answered short.
fn site_traces(c: &cascade::Cell) -> Option<Vec<cascade::Trace>> {
    let mut coarse: Vec<[i32; 3]> = Vec::new();
    c.coarse_plan(|p| coarse.push(p));
    // The scan is read back in plan order, one reply per sample row.
    let space = probe::ask(coarse, terrain_space_at)?;
    let rock: Vec<bool> = space.iter().map(|s| *s == TerrainSpace::Solid).collect();
    let free: Vec<bool> = space.iter().map(|s| *s == TerrainSpace::Air).collect();
    Some(c.traces(&rock, &free))
}

/// Every giant STANDING over the built domain, from the same resolver the
/// emit pass uses — root, terrain fit and cap competition already settled, so
/// the containment flood models exactly the bodies the world will hold. The
/// basin then decides who stays out of its water.
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
