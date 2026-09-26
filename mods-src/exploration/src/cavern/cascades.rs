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
use crate::probe::{self, Deferred, Memo, Settled, TerrainReads};

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
    for (t, &b) in traces.iter().zip(&anchor_biomes) {
        if b != ours {
            continue;
        }
        let mut plan: Vec<[i32; 3]> = Vec::new();
        t.plan(|p| plan.push(p));
        let mut reads = TerrainReads::new();
        if !reads.ask(plan) {
            return None;
        }
        // The containment proof models rock and room only, so a basin
        // meeting a fluid is not sited.
        if reads.any_fluid() {
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
