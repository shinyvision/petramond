use std::cell::RefCell;

use mod_sdk::*;

use super::cascades::giant_suppressed;
use super::claims::Emitter;
use super::{
    pick_species, ANCHOR_LATTICE, CLAIM_ROWS, GIANT_LATTICE_ONE_IN, MAX_REACH, MAX_RISE,
    PROBE_PER_CANDIDATE, SALT_GIANT,
};
use crate::content::Content;
use crate::probe::{self, Deferred, Settled};
use crate::shroom::{Giant, Part};

type Root = [i32; 3];
type Key = (u32, u8, [i32; 3]);
const CAPACITY: usize = 8192;

thread_local! {
    static ROOTS: RefCell<Settled<Key, Option<Root>>> = RefCell::new(Settled::new(CAPACITY));
}

pub(super) const COMPETE_PAD: i32 = 20;

#[derive(Clone)]
pub(super) struct Candidate {
    pub(super) x: i32,
    pub(super) z: i32,
    pub(super) cell_floor_y: i32,
    pub(super) lat: [i32; 3],
    pub(super) giant: Giant,
}

impl Candidate {
    pub(super) fn cell_top_y(&self) -> i32 {
        self.cell_floor_y + ANCHOR_LATTICE - 1
    }
}

pub(super) fn emit(
    content: &Content,
    out: &mut Emitter,
    seed: u32,
    ours: u8,
) -> Result<(), Deferred> {
    let origin = out.ctx().origin_world();
    let sect_hi = [origin[0] + 15, origin[1] + 15 + CLAIM_ROWS, origin[2] + 15];
    for (c, root) in standing_giants_over(seed, ours, origin, sect_hi) {
        if !reaches_section(&c, origin) {
            continue;
        }
        if giant_suppressed(seed, ours, &c)? {
            continue;
        }
        let [wx, wy, wz] = root;
        let species = pick_species(content, seed, wx, wy, wz);
        c.giant.emit(|dx, dy, dz, part| {
            let block = match part {
                Part::Stem => content.stem,
                Part::Cap | Part::Gill => species.cap,
            };
            out.push_if_clear([wx + dx, wy + dy, wz + dz], block);
        });
    }
    Ok(())
}

pub(super) fn could_reach(seed: u32, origin: [i32; 3]) -> bool {
    let lo = |v: i32, pad: i32| (v - pad).div_euclid(ANCHOR_LATTICE);
    let hi = |v: i32, pad: i32| (v + 16 + pad).div_euclid(ANCHOR_LATTICE);
    for lz in lo(origin[2], MAX_REACH)..=hi(origin[2], MAX_REACH) {
        for lx in lo(origin[0], MAX_REACH)..=hi(origin[0], MAX_REACH) {
            for ly in lo(origin[1], MAX_RISE)..=hi(origin[1], 0) {
                if roll_giant(seed, lx, ly, lz).is_some_and(|c| reaches_section(&c, origin)) {
                    return true;
                }
            }
        }
    }
    false
}

fn reaches_section(c: &Candidate, origin: [i32; 3]) -> bool {
    let r = c.giant.reach();
    let overlaps = |a: i32, lo: i32, len: i32, pad: i32| a + pad >= lo && a - pad < lo + len;
    overlaps(c.x, origin[0], 16, r)
        && overlaps(c.z, origin[2], 16, r)
        && c.cell_floor_y < origin[1] + CLAIM_ROWS
        && c.cell_top_y() + c.giant.rise() >= origin[1]
}

pub(super) fn roll_giant(seed: u32, lx: i32, ly: i32, lz: i32) -> Option<Candidate> {
    let mut rng = GenRng::positional(seed, SALT_GIANT, lx, ly, lz);
    if rng.next_i32(0, GIANT_LATTICE_ONE_IN - 1) != 0 {
        return None;
    }
    let mut jitter = || rng.next_i32(0, ANCHOR_LATTICE - 1);
    let x = lx * ANCHOR_LATTICE + jitter();
    let z = lz * ANCHOR_LATTICE + jitter();
    let scale = rng.next_i32(0, 255) as u8;
    Some(Candidate {
        x,
        z,
        cell_floor_y: ly * ANCHOR_LATTICE,
        lat: [lx, ly, lz],
        giant: Giant::roll(&mut rng, scale),
    })
}

/// The anchor cells that can stand a giant crossing `lo..=hi`, and the world box they span.
fn roll_box(lo: [i32; 3], hi: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    let l = ANCHOR_LATTICE;
    let first = |v: i32| (v - (l - 1)).div_euclid(l) * l;
    let last = |v: i32| v.div_euclid(l) * l + l - 1;
    (
        [
            first(lo[0] - MAX_REACH),
            first(lo[1] - MAX_RISE),
            first(lo[2] - MAX_REACH),
        ],
        [
            last(hi[0] + MAX_REACH),
            last(hi[1]),
            last(hi[2] + MAX_REACH),
        ],
    )
}

/// Every candidate rolled in the anchor cells `roll_box` spans, skipping the cells `leaves`
/// proves hold none of the biome: a candidate is gated on its own cell's biome before it can
/// stand or compete, so those never mattered, and the rolls are positional.
fn giant_rolls_over(seed: u32, lo: [i32; 3], hi: [i32; 3], leaves: &LeafMask) -> Vec<Candidate> {
    let l = ANCHOR_LATTICE;
    let (box_lo, box_hi) = roll_box(lo, hi);
    let mut out = Vec::new();
    for lz in box_lo[2] / l..=box_hi[2] / l {
        for lx in box_lo[0] / l..=box_hi[0] / l {
            let column = leaves.column(lx * l, lz * l);
            if !column.any() {
                continue;
            }
            for ly in box_lo[1] / l..=box_hi[1] / l {
                if !column.may_hold(ly * l) {
                    continue;
                }
                if let Some(c) = roll_giant(seed, lx, ly, lz) {
                    out.push(c);
                }
            }
        }
    }
    out
}

pub(super) fn could_reach_box(c: &Candidate, lo: [i32; 3], hi: [i32; 3]) -> bool {
    let r = c.giant.reach();
    c.x + r >= lo[0]
        && c.x - r <= hi[0]
        && c.z + r >= lo[2]
        && c.z - r <= hi[2]
        && c.cell_floor_y <= hi[1]
        && c.cell_top_y() + c.giant.rise() >= lo[1]
}

/// A candidate's cap centre and radius, as [`could_beat`] reads them.
type Footprint = (i32, i32, i32);

fn footprint(c: &Candidate) -> Footprint {
    let (ax, az, ar) = c.giant.cap_footprint();
    (c.x + ax, c.z + az, ar)
}

#[cfg(test)]
pub(super) fn could_beat(a: &Candidate, b: &Candidate) -> bool {
    could_beat_with(a.lat, footprint(a), b.lat, footprint(b))
}

fn could_beat_with(a_lat: [i32; 3], a: Footprint, b_lat: [i32; 3], b: Footprint) -> bool {
    if a_lat >= b_lat {
        return false;
    }
    let (dx, dz) = (a.0 - b.0, a.1 - b.1);
    let r = a.2 + b.2;
    dx * dx + dz * dz < r * r
}

fn highest_floor(space: &[TerrainSpace]) -> Option<usize> {
    (1..space.len())
        .rev()
        .find(|&k| space[k] == TerrainSpace::Air && space[k - 1] == TerrainSpace::Solid)
}

/// Every giant that actually STANDS over the inclusive world box: rolled,
/// biome-gated, rooted on the positional terrain, passing the all-or-nothing
/// terrain-fit test, and surviving cap competition. Returned with its root.
/// Authoritative only for bodies crossing `[lo, hi]` — callers filter to that.
///
/// The three verdicts:
/// - ROOT: the highest open cell resting on rock inside the anchor's own cell.
/// - FIT: every cell of the sparse body skeleton (`Giant::fit_probes`) must be
///   open terrain. A mushroom a wall or ceiling would clip is not placed at
///   all, so every section makes the same placement decision.
/// - COMPETITION: of two viable mushrooms whose caps interpenetrate, the
///   lexicographically smaller anchor stands. Deliberately pairwise against
///   VIABILITY rather than a greedy chain: a chain's outcome depends on
///   candidates arbitrarily far away, and this must resolve identically from
///   every section that can see either mushroom. The price is that a beaten
///   mushroom still bars its own victims — both of a losing pair can fall.
///
/// WATER-BLIND by construction: cascades call this to model their intruders,
/// so consulting cascade state here would re-enter the per-cell memo while it
/// is being computed. The cascade suppresses in-water giants itself
/// (`Feature::suppressed`) and the emit pass applies that verdict afterwards —
/// removal only, never revival, so the world can only LOSE bodies the
/// containment proof modelled as solid, never gain one.
pub(super) fn standing_giants_over(
    seed: u32,
    ours: u8,
    lo: [i32; 3],
    hi: [i32; 3],
) -> Vec<(Candidate, [i32; 3])> {
    let (roll_lo, roll_hi) = (
        [lo[0] - COMPETE_PAD, lo[1] - MAX_RISE, lo[2] - COMPETE_PAD],
        [hi[0] + COMPETE_PAD, hi[1] + MAX_RISE, hi[2] + COMPETE_PAD],
    );
    let (box_lo, box_hi) = roll_box(roll_lo, roll_hi);
    let leaves = underground_biome_leaves(box_lo, box_hi, ours);
    let rolled = giant_rolls_over(seed, roll_lo, roll_hi, &leaves);
    let primary: Vec<_> = rolled
        .iter()
        .enumerate()
        .filter_map(|(i, c)| could_reach_box(c, lo, hi).then_some(i))
        .collect();
    if primary.is_empty() {
        return Vec::new();
    }
    let feet: Vec<Footprint> = rolled.iter().map(footprint).collect();
    let mut is_primary = vec![false; rolled.len()];
    for &i in &primary {
        is_primary[i] = true;
    }
    let cands: Vec<_> = rolled
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            (is_primary[i]
                || primary
                    .iter()
                    .any(|&j| could_beat_with(c.lat, feet[i], rolled[j].lat, feet[j])))
            .then_some(c.clone())
        })
        .collect();
    let Some(viable) = viable_roots(seed, ours, &cands) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    'next: for &(i, root) in &viable {
        if !could_reach_box(&cands[i], lo, hi) {
            continue;
        }
        for &(j, rj) in &viable {
            if j != i && beats(&cands[j], rj, &cands[i], root) {
                continue 'next;
            }
        }
        out.push((cands[i].clone(), root));
    }
    out
}

pub(super) fn beats(a: &Candidate, ra: [i32; 3], b: &Candidate, rb: [i32; 3]) -> bool {
    if a.lat >= b.lat {
        return false;
    }
    let (ax, az, ar) = a.giant.cap_footprint();
    let (bx, bz, br) = b.giant.cap_footprint();
    let (dx, dz) = (ra[0] + ax - rb[0] - bx, ra[2] + az - rb[2] - bz);
    let r = ar + br;
    if dx * dx + dz * dz >= r * r {
        return false;
    }
    let (alo, ahi) = a.giant.cap_levels();
    let (blo, bhi) = b.giant.cap_levels();
    ra[1] + alo <= rb[1] + bhi && rb[1] + blo <= ra[1] + ahi
}

fn memo_key(ours: u8, lat: [i32; 3]) -> Vec<u8> {
    let mut w = ByteWriter::with_capacity(14);
    w.raw(&[b'g', ours]);
    w.i32x3(lat);
    w.finish()
}

fn encode_root(root: Option<Root>) -> Vec<u8> {
    let mut w = ByteWriter::with_capacity(13);
    match root {
        None => w.raw(&[0]),
        Some(p) => {
            w.raw(&[1]);
            w.i32x3(p);
        }
    }
    w.finish()
}

fn decode_root(bytes: &[u8]) -> Option<Option<Root>> {
    let mut r = ByteReader::new(bytes);
    match r.take(1)? {
        [0] => Some(None),
        [1] => Some(Some(r.i32x3()?)),
        _ => None,
    }
}

fn viable_roots(seed: u32, ours: u8, candidates: &[Candidate]) -> Option<Vec<(usize, Root)>> {
    let mut roots = vec![None; candidates.len()];
    let missing = ROOTS.with(|cache| {
        let cache = cache.borrow();
        let mut missing = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            match cache.get(&(seed, ours, candidate.lat)) {
                Some(root) => roots[index] = root,
                None => missing.push(index),
            }
        }
        missing
    });
    if !missing.is_empty() {
        let mut resolved: Vec<(usize, Option<Root>)> = Vec::with_capacity(missing.len());
        let mut unresolved = Vec::new();
        let keys = missing
            .iter()
            .map(|&i| memo_key(ours, candidates[i].lat))
            .collect();
        for (&i, shared) in missing.iter().zip(probe::lookup_many(memo_get_many, keys)) {
            match shared.as_deref().and_then(decode_root) {
                Some(root) => resolved.push((i, root)),
                None => unresolved.push(i),
            }
        }
        if !unresolved.is_empty() {
            let batch: Vec<_> = unresolved.iter().map(|&i| &candidates[i]).collect();
            let decisions = probe_roots(ours, &batch)?;
            for (&i, root) in unresolved.iter().zip(decisions) {
                memo_put(&memo_key(ours, candidates[i].lat), encode_root(root));
                resolved.push((i, root));
            }
        }
        ROOTS.with(|cache| {
            let mut cache = cache.borrow_mut();
            for (index, root) in resolved {
                cache.insert((seed, ours, candidates[index].lat), root);
                roots[index] = root;
            }
        });
    }
    Some(
        roots
            .into_iter()
            .enumerate()
            .filter_map(|(i, root)| root.map(|p| (i, p)))
            .collect(),
    )
}

fn probe_roots(ours: u8, cands: &[&Candidate]) -> Option<Vec<Option<Root>>> {
    let gate: Vec<[i32; 3]> = cands
        .iter()
        .map(|c| [c.x, c.cell_floor_y + ANCHOR_LATTICE / 2, c.z])
        .collect();
    let biomes = probe::ask(gate, underground_biome_at)?;
    let mut plan: Vec<[i32; 3]> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        if biomes[i] != ours {
            continue;
        }
        spans.push((i, plan.len()));
        for wy in (c.cell_floor_y - 1)..=c.cell_top_y() {
            plan.push([c.x, wy, c.z]);
        }
    }
    let space = probe::ask(plan, terrain_space_at)?;
    let mut rooted: Vec<(usize, [i32; 3])> = Vec::new();
    for &(i, start) in &spans {
        let c = &cands[i];
        if let Some(k) = highest_floor(&space[start..start + PROBE_PER_CANDIDATE]) {
            rooted.push((i, [c.x, c.cell_floor_y - 1 + k as i32, c.z]));
        }
    }
    let mut plan: Vec<[i32; 3]> = Vec::new();
    let mut fit_at: Vec<usize> = Vec::with_capacity(rooted.len());
    for &(i, root) in &rooted {
        fit_at.push(plan.len());
        cands[i].giant.fit_probes(|dx, dy, dz| {
            plan.push([root[0] + dx, root[1] + dy, root[2] + dz]);
        });
    }
    let space = probe::ask(plan, terrain_space_at)?;
    let mut viable = vec![None; cands.len()];
    for (k, &(i, root)) in rooted.iter().enumerate() {
        let end = fit_at.get(k + 1).copied().unwrap_or(space.len());
        if space[fit_at[k]..end]
            .iter()
            .all(|&s| s == TerrainSpace::Air)
        {
            viable[i] = Some(root);
        }
    }
    Some(viable)
}
