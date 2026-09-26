//! Giant mushrooms: the positional rolls, the root and fit verdicts every
//! section shares, cap competition, and emission.

use std::cell::RefCell;

use mod_sdk::*;

use super::cascades::giant_suppressed;
use super::claims::Emitter;
use super::{
    pick_species, ANCHOR_LATTICE, CLAIM_ROWS, GIANT_LATTICE_ONE_IN, MAX_REACH, MAX_RISE,
    PROBE_PER_CANDIDATE, SALT_GIANT,
};
use crate::content::Content;
use crate::probe::{self, Deferred, Memo, Settled};
use crate::shroom::{Giant, Part};

type Root = [i32; 3];
/// `(seed, biome, anchor lattice cell)`.
type Key = (u32, u8, [i32; 3]);
const CAPACITY: usize = 8192;

thread_local! {
    static ROOTS: RefCell<Settled<Key, Option<Root>>> = RefCell::new(Settled::new(CAPACITY));
}

/// Cap-competition pad: the farthest apart two anchors can sit with their caps
/// still able to interpenetrate — cap radius plus the cap centre's lean offset,
/// for each of the pair. Pinned against rolled maxima by
/// `compete_pad_covers_every_rolled_cap`. The candidate sweep must extend this
/// far past a caller's own margin so a verdict is decided with the whole
/// neighbourhood present, identically from every section.
pub(super) const COMPETE_PAD: i32 = 20;

/// A rolled giant, and the vertical window its root may be found in.
///
/// The roll picks a COLUMN; the terrain picks the height. That split is the
/// whole reason mushrooms come out rooted: a rolled 3-D point inside an 8³ cell
/// only lands exactly on a cavern floor a few per cent of the time, so demanding
/// it be one throws away nearly every candidate.
#[derive(Clone)]
pub(super) struct Candidate {
    pub(super) x: i32,
    pub(super) z: i32,
    /// Floor of the anchor's own lattice cell. The root search covers the whole
    /// cell and never leaves it, so the cell that owns the roll owns the
    /// mushroom and every section derives the same one.
    pub(super) cell_floor_y: i32,
    /// The anchor's lattice cell — the giant's IDENTITY, which is what a
    /// cascade's suppression list names.
    pub(super) lat: [i32; 3],
    pub(super) giant: Giant,
}

impl Candidate {
    pub(super) fn cell_top_y(&self) -> i32 {
        self.cell_floor_y + ANCHOR_LATTICE - 1
    }
}

/// Emit every standing giant whose body reaches this section, unless a
/// cascade suppresses it.
///
/// The resolver already settled root, terrain fit and cap competition —
/// identically for every section, water-blind. The one verdict applied here
/// is the cascades': a basin is terrain, and a giant standing in its water
/// (or breaking its containment proof) is suppressed by it
/// (`Feature::suppressed`) — a giant never vetoes a basin.
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

/// Could any rolled giant reach the section at `origin` at all? A roll sweep
/// only — no host call is paid before it says yes.
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

/// Whether a mushroom rooted anywhere in `c`'s search window can put a cell
/// inside the 16³ section at `origin`. The root height is not known yet, so the
/// vertical test spans the whole window, bounded by the ROLLED giant's own
/// reach and rise rather than by the scan margins — the margins have to cover
/// the worst roll, this candidate only has to cover itself.
fn reaches_section(c: &Candidate, origin: [i32; 3]) -> bool {
    let r = c.giant.reach();
    let overlaps = |a: i32, lo: i32, len: i32, pad: i32| a + pad >= lo && a - pad < lo + len;
    overlaps(c.x, origin[0], 16, r)
        && overlaps(c.z, origin[2], 16, r)
        // The margin rows are kept for the same reason a curtain scans them: a
        // mushroom standing just over our roof has to stop a run hanging in
        // through it, and only a giant we RESERVE can do that. It writes
        // nothing here — `push_if_clear` refuses cells the section does not own.
        && c.cell_floor_y < origin[1] + CLAIM_ROWS
        && c.cell_top_y() + c.giant.rise() >= origin[1]
}

/// The giant-mushroom roll one lattice cell carries, or `None`. ONE function
/// on purpose: the anchor gather and the cascade's intruder sweep must derive
/// bit-identical mushrooms or the containment flood models a world that is
/// not the one being written.
///
/// Drawn into locals in this order, on purpose: the stream is the world's
/// content, so it must not depend on where a struct literal happens to list
/// its fields.
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

/// Every giant roll whose BODY could intersect the inclusive world box: roots
/// within the worst-case reach horizontally, and within the worst-case rise
/// below it (a mushroom's cells all sit at or above its root).
fn giant_rolls_over(seed: u32, lo: [i32; 3], hi: [i32; 3]) -> Vec<Candidate> {
    let l = ANCHOR_LATTICE;
    let cells = |a: i32, b: i32| (a - (l - 1)).div_euclid(l)..=b.div_euclid(l);
    let mut out = Vec::new();
    for lz in cells(lo[2] - MAX_REACH, hi[2] + MAX_REACH) {
        for lx in cells(lo[0] - MAX_REACH, hi[0] + MAX_REACH) {
            for ly in cells(lo[1] - MAX_RISE, hi[1]) {
                if let Some(c) = roll_giant(seed, lx, ly, lz) {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// The highest FREE cell resting on ROCK, over a column probe whose first
/// slot is the support cell UNDER the window — the root rule every giant
/// stands on, shared so no second derivation can drift from it. A fluid cell
/// is neither.
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
///   open terrain. A mushroom a wall or ceiling would clip is not placed AT
///   ALL — a clipped fragment was the bug, and one section placing what
///   another rejects would be the worse bug.
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
    // Competitors of a body-crossing candidate can root up to COMPETE_PAD
    // beyond it horizontally and a full rise beyond it vertically; sweep the
    // padded box so every verdict below sees its whole neighbourhood.
    let cands = giant_rolls_over(
        seed,
        [lo[0] - COMPETE_PAD, lo[1] - MAX_RISE, lo[2] - COMPETE_PAD],
        [hi[0] + COMPETE_PAD, hi[1] + MAX_RISE, hi[2] + COMPETE_PAD],
    );
    if cands.is_empty() {
        return Vec::new();
    }
    let Some(viable) = viable_roots(seed, ours, &cands) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    'next: for &(i, root) in &viable {
        for &(j, rj) in &viable {
            if j != i && beats(&cands[j], rj, &cands[i], root) {
                continue 'next;
            }
        }
        out.push((cands[i].clone(), root));
    }
    out
}

/// Does `a` beat `b` in cap competition? Smaller anchor key, and the two caps
/// actually interpenetrate — centre distance under the radius sum AND cap
/// level spans intersecting. Touching (distance equal to the sum) shares no
/// cell and both stand.
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

/// One anchor cell's verdict in the shared memo (scoped by the host to this
/// mod and the world seed).
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

/// The root of every candidate that has one, as `(candidate index, root)`:
/// from this worker's cache, then the shared memo, and only then a probe —
/// whose verdicts are published for everyone. `None` when a host reply came
/// back short: a failed reply is not a positional rejection and must not
/// persist.
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
        for (&i, shared) in missing.iter().zip(probe::lookup_many(Memo::HOST, keys)) {
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

/// Root and fit verdicts for candidates nobody has settled yet, in three
/// crossings for the whole batch: a biome gate, the root columns, and the
/// fit skeletons.
fn probe_roots(ours: u8, cands: &[&Candidate]) -> Option<Vec<Option<Root>>> {
    // Biome gate at the middle of the root window, the same fixed point every
    // other pass asks about.
    let gate: Vec<[i32; 3]> = cands
        .iter()
        .map(|c| [c.x, c.cell_floor_y + ANCHOR_LATTICE / 2, c.z])
        .collect();
    let biomes = probe::ask(gate, underground_biome_at)?;
    // Roots for the biome survivors: one column span each, one crossing.
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
    // The fit skeletons, one more crossing for all of them together.
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
        // Every cell the body fills must be free ROOM; a fluid is not room.
        if space[fit_at[k]..end]
            .iter()
            .all(|&s| s == TerrainSpace::Air)
        {
            viable[i] = Some(root);
        }
    }
    Some(viable)
}
