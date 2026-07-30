//! Dressing a mushroom cavern.
//!
//! THIS MODULE DOES NOT SHAPE ANYTHING. The cathedral-scale room is declared by
//! the `chamber` clause on this pack's `underground_biomes.json` row, so the
//! engine's own carvers open it and natural tunnels blend into it. The pack
//! used to carve that room itself, as air writes two stages after the terrain
//! was carved, and a stamp against finished geometry left every tunnel that
//! reached it ending at a flat face. Nothing here should ever go back to
//! stamping a ROOM as air.
//!
//! (`cascade.rs` does write air, and the distinction is the whole point: it
//! cuts spill notches a few cells wide through terrace lips its containment
//! flood has already proved out against the surrounding terrain — slots for
//! water to pour through, never a room, so the cut cannot union with existing
//! geometry the way the stamped hall sheared tunnels off.)
//!
//! SEAM CONTRACT. Sections generate independently, in any order, on any thread,
//! and the engine clips each dispatch's writes to its own section. So every
//! decision here is a pure function of `(world seed, our own salt, ANCHOR
//! coordinates)` — never of visit order, never of state carried between
//! dispatches. A giant mushroom rooted in a neighbouring column is re-derived
//! bit-identically by every section its cap reaches, which is what makes it
//! come out whole instead of sliced at the boundary.
//!
//! WHICH READS MAY DECIDE. `GenCtx::block` answers only for cells inside the
//! DISPATCHING section, so it can never carry an acceptance decision about a
//! structure that spans sections: the owner would reject a mushroom its
//! neighbours accept, and the neighbours would still write their share — caps
//! floating in mid-air with no stem. Anything that decides WHETHER a mushroom
//! exists therefore reads `terrain_solid_at`, which is positional and answers
//! the same in every section.
//!
//! The same read is what makes DRESSING reach the section planes. A decoration
//! is a single cell, owned by exactly one section, so it needs no cross-section
//! agreement — but it does need the truth about its own support and roof, and
//! those lie in the NEIGHBOURING section on local rows 0 and 15. Treating an
//! unseen neighbour as "not solid" left every `y % 16 == 0` plane undressed,
//! the world floor at the bottom of the biome band worst of all: the largest,
//! flattest floor a player ever walks on, and the only one with no flora. Every
//! probe that reaches OUTSIDE the section is therefore positional; the ones
//! that test a cell this section owns stay on `GenCtx::block`, where
//! disagreement is impossible by construction.
//!
//! HOST-CALL BUDGET. Both queries are ABI crossings, not field samplers. Every
//! candidate is filtered by the cheap positional roll first; only the survivors
//! — a handful per section — are batched, into exactly two calls: one biome
//! query for every candidate, then one terrain query over the anchor columns
//! and the boundary cells the biome kept. A per-cell query here would trip the
//! mod watchdog on the first cavern.
//!
//! A CASCADE runs its own crossings on its own memoized per-cell pipeline
//! (`cascade_cell`), never riding the shared batches: rarity roll (free),
//! biome pre-gate (one tiny crossing), coarse height scan (two batches), and
//! only for a cell whose floor offers a real contour edge the band probe and
//! the giant-intruder sweep. Cascades resolve from the terrain alone and are
//! AUTHORITATIVE: no giant stands in a basin's water, shallow or deep — one
//! in it, on it, or breaking its proof is suppressed — and the giant pass
//! queries that decision positionally (`giant_suppressed`). A giant never
//! vetoes a basin,
//! and the ordering is real in every section because it is a positional
//! query, not an emission-order accident.

use std::rc::Rc;

use mod_sdk::*;

use crate::cascade;
use crate::content::{Content, Species};
use crate::shroom::{self, Giant, Part};

/// Frozen positional-RNG salts. Changing one reshuffles that stream in every
/// existing world, so they are append-only in practice.
const SALT_GIANT: u64 = 0x0E58_1000_0000_0001;
const SALT_GROUND: u64 = 0x0E58_1000_0000_0002;
const SALT_CEILING: u64 = 0x0E58_1000_0000_0003;
const SALT_SPECIES: u64 = 0x0E58_1000_0000_0004;
const SALT_VINE_BLOOM: u64 = 0x0E58_1000_0000_0005;
const SALT_PATCH: u64 = 0x0E58_1000_0000_0006;

/// One in N LATTICE CELLS carries a giant-mushroom candidate. Balance data:
/// candidates are filtered again by the biome and by needing a real floor, so
/// this is much denser than the mushrooms that survive. A cavern's impact comes
/// from a stand of landmark specimens, not a forest.
const GIANT_LATTICE_ONE_IN: i32 = 5;
/// Ground flora density is no longer a flat per-cell chance — see `patch_at`,
/// which grows it in colonies. Ceilings stay a plain per-cell roll: a curtain
/// hangs where a roof happens to be, and clumping those reads as a mistake.
const VINE_PER_MILLE: i32 = 55;
/// Per-mille chance that one CELL of a curtain is the luminous variant. Picked
/// per cell rather than per curtain so a run reads as a plain strand with the
/// odd flowering segment on it; at this rate about a third of curtains carry
/// one. Blooms are real light sources, and the cavern's mood comes from sparse
/// strong sources with darkness between them, so this wants to stay low.
const VINE_BLOOM_PER_MILLE: i32 = 100;
const VINE_MAX_LEN: i32 = 7;
/// Rows ABOVE this section that are scanned for vine roots. A curtain hangs
/// DOWN, so a root this far over our roof still drapes into us — and the
/// section that owns the root cannot write our cells, so if we do not scan for
/// it the curtain simply stops at the section plane. Cells over our roof are
/// read from the positional terrain; `GenCtx` cannot see them.
const CEILING_MARGIN: i32 = VINE_MAX_LEN - 1;
/// Terrain cells probed per margin column: every row a root may sit on, plus
/// the roof over the highest of them. The real path derives this bound from
/// the scan itself, so only the budget and invariant tests name it.
#[cfg(test)]
const PROBE_PER_MARGIN: usize = CEILING_MARGIN as usize + 1;

/// The largest horizontal reach any rolled mushroom can have. Anchors are
/// scanned this far outside the section so a cap overhanging the boundary is
/// still re-derived here. MUST be >= the worst case `Giant::reach()`.
const MAX_REACH: i32 = 14;
/// Vertical span a mushroom can occupy above its root, for the same reason.
const MAX_RISE: i32 = 26;

/// The ABI's per-call element cap. Batches are SPLIT at it rather than
/// truncated: a truncated batch drops candidates by list position, which both
/// makes the result depend on how many other candidates happened to roll and
/// lets a cell that is going to lose to a giant displace a legitimate floor.
const ABI_BATCH_MAX: usize = 4096;

/// Giant mushrooms anchor on a COARSE LATTICE rather than per cell.
///
/// This is a performance contract, not a style choice. Per-cell candidacy over
/// the margin window is ~44x44x42 positional rolls for every section the
/// streamer generates — hundreds of thousands of hashes per second during
/// normal flight, for a biome that is absent from almost every section. On the
/// lattice it is a few hundred. It also spaces the mushrooms out, which reads
/// better than the clumping a per-cell roll produces.
const ANCHOR_LATTICE: i32 = 8;

/// Terrain cells probed per candidate: its whole lattice cell, plus the support
/// cell below the lowest one so every floor test is a pair inside the batch.
const PROBE_PER_CANDIDATE: usize = ANCHOR_LATTICE as usize + 1;

/// Highest world Y this feature can write anything at: a giant anchored at the
/// very top of the biome's declared depth band, at full rise. The engine
/// dispatches every registered feature for EVERY section it generates, so
/// bounding the pack's own altitude is the pack's job — without this the whole
/// gather runs, provably fruitlessly, for every section from bedrock to sky.
const TOP_CONTENT_Y: i32 = crate::BIOME_TOP_Y + MAX_RISE;

/// A rolled giant, and the vertical window its root may be found in.
///
/// The roll picks a COLUMN; the terrain picks the height. That split is the
/// whole reason mushrooms come out rooted: a rolled 3-D point inside an 8³ cell
/// only lands exactly on a cavern floor a few per cent of the time, so demanding
/// it be one throws away nearly every candidate.
#[derive(Clone)]
pub(crate) struct Candidate {
    pub(crate) x: i32,
    pub(crate) z: i32,
    /// Floor of the anchor's own lattice cell. The root search covers the whole
    /// cell and never leaves it, so the cell that owns the roll owns the
    /// mushroom and every section derives the same one.
    pub(crate) cell_floor_y: i32,
    /// The anchor's lattice cell — the giant's IDENTITY, which is what a
    /// cascade's suppression list names.
    pub(crate) lat: [i32; 3],
    pub(crate) giant: Giant,
}

impl Candidate {
    fn cell_top_y(&self) -> i32 {
        self.cell_floor_y + ANCHOR_LATTICE - 1
    }
}

/// One rolled decoration cell inside this section, with whatever `GenCtx` could
/// tell us about the two cells that classify it.
///
/// `None` means the neighbour lies in the section above or below. That happens
/// on exactly two rows — local 0 has no readable support, local 15 no readable
/// roof — and it is resolved by a positional probe, never by assuming the
/// neighbour is air.
struct Dress {
    p: [i32; 3],
    below: Option<bool>,
    above: Option<bool>,
    /// Index into the terrain reply of whichever neighbour `GenCtx` could not
    /// see, once the biome has kept this cell.
    probe: Option<usize>,
}

impl Dress {
    /// The one neighbour this cell cannot classify itself from. A 16-tall
    /// section can never lack both.
    fn unseen(&self) -> Option<[i32; 3]> {
        match (self.below, self.above) {
            (None, _) => Some([self.p[0], self.p[1] - 1, self.p[2]]),
            (_, None) => Some([self.p[0], self.p[1] + 1, self.p[2]]),
            _ => None,
        }
    }

    /// Resting on rock, once the probe (if any) has answered.
    fn on_rock(&self, solid: &[bool]) -> bool {
        self.below
            .unwrap_or_else(|| self.probe.is_some_and(|i| solid[i]))
    }

    /// Under a roof, same.
    fn under_roof(&self, solid: &[bool]) -> bool {
        self.above
            .unwrap_or_else(|| self.probe.is_some_and(|i| solid[i]))
    }
}

/// A vine root sitting ABOVE this section's roof, and the column probe block
/// that answers for it. The section that owns the root cannot write our cells,
/// so a curtain only crosses the plane because we re-derive its root here.
struct Margin {
    p: [i32; 3],
    /// Index into `margin_cols` — roots in one column share one probe block.
    col: usize,
}

/// A column of terrain cells over this section's roof, and how many of them the
/// roots filed against it actually read: rows `16 ..= 16 + k + 1` for the
/// highest root at `16 + k`. Probing the whole margin band for every column
/// would be `PROBE_PER_MARGIN` point queries where two usually suffice.
struct MarginCol {
    xz: [i32; 2],
    rows: usize,
}

pub fn generate(content: &Content, ctx: &GenCtx) -> Vec<GenWrite> {
    let origin = ctx.origin_world();
    if origin[1] > TOP_CONTENT_Y {
        return Vec::new();
    }
    let Some(ours) = biome_id() else {
        return Vec::new();
    };
    let seed = ctx.seed();

    // --- bounded biome gates, before anything is rolled -----------------
    // Worldgen dispatches this feature for EVERY section in its altitude band,
    // and a cavern is a rare event, so nearly every dispatch is a section with
    // none of our territory in it. Rolling the candidates first and asking the
    // biome per candidate paid the whole gather — plus a few hundred per-cell
    // crossings — to learn that. These ask once per PASS, over exactly the reach
    // that pass can be authorised from, at a cost that does not grow with the
    // candidate count. Conservative in the safe direction: a `true` only means
    // "do the real work", never "place something".
    //
    // Two tiers, because the two passes reach very differently and one box wide
    // enough for both is wide enough to be true nearly everywhere: a giant is
    // rolled on a lattice up to a full reach/rise away, while a floor plant or a
    // curtain root is a cell of this section (plus the rows a curtain hangs
    // through). The giant box CONTAINS the dressing box, so a `false` there
    // settles both.
    let ours_in_reach = ours_in_box(ours, origin, GIANT_PAD);
    let ours_here = ours_in_reach && ours_in_box(ours, origin, DRESS_PAD);

    // --- gather candidates (no host calls yet) --------------------------
    // Giants resolve through the shared, water-blind `standing_giants_over`,
    // so this pass and the cascades' intruder model can never drift. The roll
    // sweep here only answers "could one reach this section at all", before
    // any host call is paid for it.
    let any_giant = ours_in_reach && {
        let lo = |v: i32, pad: i32| (v - pad).div_euclid(ANCHOR_LATTICE);
        let hi = |v: i32, pad: i32| (v + 16 + pad).div_euclid(ANCHOR_LATTICE);
        let mut found = false;
        'sweep: for lz in lo(origin[2], MAX_REACH)..=hi(origin[2], MAX_REACH) {
            for lx in lo(origin[0], MAX_REACH)..=hi(origin[0], MAX_REACH) {
                for ly in lo(origin[1], MAX_RISE)..=hi(origin[1], 0) {
                    if let Some(c) = roll_giant(seed, lx, ly, lz) {
                        if reaches_section(&c, origin) {
                            found = true;
                            break 'sweep;
                        }
                    }
                }
            }
        }
        found
    };

    // Cascades run their own per-cell, per-worker-cached pipeline — see
    // `cascade_cell` — so the shared batches below carry no cascade slots.
    let mut features: Vec<Rc<cascade::Feature>> = Vec::new();
    for cell in cascade::cells_overlapping(origin, CLAIM_ROWS) {
        if let Some(f) = cascade_cell(seed, ours, cell) {
            features.push(f);
        }
    }
    let (floors, ceilings, margins, margin_cols) = if ours_here {
        gather_dressing(ctx, seed)
    } else {
        Default::default()
    };

    if !any_giant
        && features.is_empty()
        && floors.is_empty()
        && ceilings.is_empty()
        && margins.is_empty()
    {
        return Vec::new();
    }

    // --- ONE batched biome query ----------------------------------------
    // Giant candidates gate their own biome inside `standing_giants_over`;
    // this batch carries only the dressing.
    let n_f = floors.len();
    let n_c = ceilings.len();
    let n_m = margins.len();
    let mut query: Vec<[i32; 3]> = Vec::with_capacity(n_f + n_c + n_m);
    query.extend(floors.iter().map(|d| d.p));
    query.extend(ceilings.iter().map(|d| d.p));
    // A margin root is asked about at the ROOT, exactly as an in-section root
    // is: both sections deriving one curtain must gate it the same way, or a
    // curtain near a territory edge comes out in halves.
    query.extend(margins.iter().map(|m| m.p));
    let want = query.len();
    let biomes = batched(query, underground_biome_at);
    if biomes.len() != want {
        return Vec::new();
    }
    let mine = |i: usize| biomes[i] == ours;

    // --- ONE batched terrain query --------------------------------------
    // The support/roof cells the boundary rows cannot see, and the rows over
    // our roof a curtain may hang in from. Unanimous across sections because
    // it reads the positional terrain, never this section's snapshot.
    let mut probe: Vec<[i32; 3]> = Vec::new();
    let mut floors = floors;
    let mut ceilings = ceilings;
    // A floor cell only ever needs its SUPPORT; asking about a roof it does not
    // read would buy a crossing for nothing.
    for (i, d) in floors.iter_mut().enumerate() {
        if mine(i) && d.below.is_none() {
            d.probe = Some(probe.len());
            probe.push([d.p[0], d.p[1] - 1, d.p[2]]);
        }
    }
    for (i, d) in ceilings.iter_mut().enumerate() {
        if !mine(n_f + i) {
            continue;
        }
        if let Some(cell) = d.unseen() {
            d.probe = Some(probe.len());
            probe.push(cell);
        }
    }
    // A margin column is only worth probing once one of its roots is in the
    // biome — otherwise every deep cave in the world would pay seven terrain
    // point queries per open roof column.
    let mut col_probe: Vec<Option<usize>> = vec![None; margin_cols.len()];
    for (i, m) in margins.iter().enumerate() {
        if !mine(n_f + n_c + i) {
            continue;
        }
        if col_probe[m.col].is_none() {
            col_probe[m.col] = Some(probe.len());
            let MarginCol { xz: [x, z], rows } = margin_cols[m.col];
            for k in 0..rows {
                probe.push([x, origin[1] + 16 + k as i32, z]);
            }
        }
    }
    let want = probe.len();
    let solid = batched(probe, terrain_solid_at);
    if solid.len() != want {
        return Vec::new();
    }

    let mut out: Vec<GenWrite> = Vec::new();
    let mut claims = Claims::default();

    // --- giant mushrooms -------------------------------------------------
    // FIRST, and that ordering is load-bearing: a stem's base cell is the
    // highest open cell resting on rock, which is exactly what a floor flower
    // asks for. Emitting the structure first and recording its claims is what
    // stops the flower replacing the stem.
    //
    // The resolver already settled root, terrain fit and cap competition —
    // identically for every section, water-blind. The one verdict applied
    // here is the cascades': a basin is terrain, and a giant standing in its
    // water (or breaking its containment proof) is suppressed by it
    // (`Feature::suppressed`) — a giant never vetoes a basin.
    let sect_hi = [origin[0] + 15, origin[1] + 15 + CLAIM_ROWS, origin[2] + 15];
    for (c, root) in standing_giants_over(seed, ours, origin, sect_hi) {
        if !reaches_section(&c, origin) {
            continue;
        }
        if giant_suppressed(seed, ours, &c) {
            continue;
        }
        let [wx, wy, wz] = root;
        let species = pick_species(content, seed, wx, wy, wz);
        c.giant.emit(|dx, dy, dz, part| {
            let block = match part {
                Part::Stem => content.stem,
                Part::Cap | Part::Gill => species.cap,
            };
            push_if_clear(
                ctx,
                &mut claims,
                &mut out,
                [wx + dx, wy + dy, wz + dz],
                block,
            );
        });
    }

    // --- cascades ----------------------------------------------------------
    // AFTER the giants and BEFORE the dressing. The write order is claims
    // plumbing, not precedence: the basin already decided which giants STAND
    // clear of its water and which are suppressed, and its containment flood
    // modelled exactly the standing bodies as solid — so letting a standing
    // giant's cells win the cell conflicts here is what keeps the world equal
    // to the proof. Before the dressing because the reserves are what keep a
    // flower off the water and a vine out of a fall.
    for f in &features {
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
            push_over_terrain(ctx, &mut claims, &mut out, cell, block);
        }
        for &cell in &f.reserves {
            reserve(ctx, &mut claims, cell);
        }
    }

    // --- floors ----------------------------------------------------------
    for (i, d) in floors.iter().enumerate() {
        if !mine(i) || !d.on_rock(&solid) {
            continue;
        }
        let p = d.p;
        let mut rng = GenRng::positional(seed, SALT_GROUND, p[0], p[1], p[2]);
        let roll = rng.next_i32(0, 999); // the same draw the gather pass made
        let (density, flower, salt) = patch_at(seed, p[0], p[2]);
        if roll >= density {
            continue;
        }
        // The colony's kind, with a minority of the other so an edge is not a
        // clean species boundary. Drawn from the SAME stream, after the
        // density roll, so the gather pass never has to know about it.
        let minority = rng.next_i32(0, 999) < PATCH_ADMIX_PER_MILLE;
        let species = patch_species(content, seed, salt, p);
        let block = if flower != minority {
            species.flower
        } else {
            species.sporeshroom
        };
        push_if_clear(ctx, &mut claims, &mut out, p, block);
    }

    // --- ceilings ---------------------------------------------------------
    for (i, d) in ceilings.iter().enumerate() {
        // A cell resting on rock is a FLOOR, whatever hangs over it; letting
        // both passes claim one would put two decorations in one cell.
        if !mine(n_f + i) || d.on_rock(&solid) || !d.under_roof(&solid) {
            continue;
        }
        hang_curtain(ctx, content, &mut claims, &mut out, seed, d.p, |cell| {
            is_open(ctx, cell)
        });
    }
    for (i, m) in margins.iter().enumerate() {
        if !mine(n_f + n_c + i) {
            continue;
        }
        let Some(start) = col_probe[m.col] else {
            continue;
        };
        // Probe slot `j` is the row `origin.y + 16 + j`; the root sits at `k`,
        // its support at `k - 1` (our own roof row when `k == 0`, which the
        // gather already proved open) and its roof at `k + 1`.
        let k = (m.p[1] - origin[1] - 16) as usize;
        let rock = |j: usize| solid[start + j];
        if rock(k) || !rock(k + 1) || (k > 0 && rock(k - 1)) {
            continue;
        }
        hang_curtain(ctx, content, &mut claims, &mut out, seed, m.p, |cell| {
            let j = cell[1] - origin[1] - 16;
            if j >= 0 {
                !rock(j as usize)
            } else {
                is_open(ctx, cell)
            }
        });
    }

    out
}

/// Every rolled decoration cell this section owns, plus the vine roots sitting
/// over its roof. Pure: no host calls, so the roll filters the candidate list
/// down before anything crosses the ABI.
///
/// Returns `(floors, ceilings, margin roots, the columns those roots probe)`.
fn gather_dressing(
    ctx: &GenCtx,
    seed: u32,
) -> (Vec<Dress>, Vec<Dress>, Vec<Margin>, Vec<MarginCol>) {
    let origin = ctx.origin_world();
    let (mut floors, mut ceilings) = (Vec::new(), Vec::new());
    let (mut margins, mut margin_cols) = (Vec::new(), Vec::new());
    for lz in 0..16 {
        for lx in 0..16 {
            let (x, z) = (origin[0] + lx, origin[2] + lz);
            for ly in 0..16 {
                let p = [x, origin[1] + ly, z];
                if !is_open(ctx, p) {
                    continue;
                }
                let below = neighbour_solid(ctx, p, -1);
                let above = neighbour_solid(ctx, p, 1);
                // The rolls come FIRST so an ineligible cell never costs a
                // query slot, and a cell whose classification is still open
                // (rows 0 and 15) is kept for BOTH passes rather than guessed
                // at; the probe decides which one it belongs to.
                let mut ground = GenRng::positional(seed, SALT_GROUND, p[0], p[1], p[2]);
                if ground.next_i32(0, 999) < patch_at(seed, p[0], p[2]).0 && below != Some(false) {
                    floors.push(Dress {
                        p,
                        below,
                        above,
                        probe: None,
                    });
                }
                let mut roof = GenRng::positional(seed, SALT_CEILING, p[0], p[1], p[2]);
                if roof.next_i32(0, 999) < VINE_PER_MILLE
                    && below != Some(true)
                    && above != Some(false)
                {
                    ceilings.push(Dress {
                        p,
                        below,
                        above,
                        probe: None,
                    });
                }
            }
            // A curtain rooted over our roof only reaches us THROUGH our top
            // row, so an open top row is a free and exact prefilter on the
            // whole margin scan.
            if !is_open(ctx, [x, origin[1] + 15, z]) {
                continue;
            }
            let mut col: Option<usize> = None;
            for k in 0..CEILING_MARGIN {
                let p = [x, origin[1] + 16 + k, z];
                let mut rng = GenRng::positional(seed, SALT_CEILING, p[0], p[1], p[2]);
                if rng.next_i32(0, 999) >= VINE_PER_MILLE {
                    continue;
                }
                // The run's LENGTH is the next draw on this very stream, and
                // over half of the margin band's roots stop before reaching our
                // roof. Rolling it here costs one hash and saves the root's
                // query slot and its whole probe block.
                if shroom::vine_len(&mut rng, VINE_MAX_LEN) < k + 2 {
                    continue;
                }
                let c = *col.get_or_insert_with(|| {
                    margin_cols.push(MarginCol {
                        xz: [x, z],
                        rows: 0,
                    });
                    margin_cols.len() - 1
                });
                // rows `16 ..= 16 + k` for the walk down, plus `16 + k + 1` for
                // this root's roof
                margin_cols[c].rows = margin_cols[c].rows.max(k as usize + 2);
                margins.push(Margin { p, col: c });
            }
        }
    }
    (floors, ceilings, margins, margin_cols)
}

/// Drop one curtain from `root`, writing only the cells this section owns.
///
/// `open` answers for cells the run walks through, INCLUDING ones above our
/// roof that we will not write: the run has to know whether it gets that far.
/// A curtain STOPS at the first cell it cannot have — skipping one and carrying
/// on leaves a tail with nothing above it, and a hanging block is held by the
/// cell above, so that tail falls on the first edit nearby.
///
/// A cell a mushroom has already claimed is a cell the curtain cannot have. It
/// is NOT enough to let `push_if_clear` refuse it: the snapshot still reads
/// open there (the mushroom is a pending write of this same dispatch), so the
/// run would step over the stem and carry on inside the cap.
fn hang_curtain(
    ctx: &GenCtx,
    content: &Content,
    claims: &mut Claims,
    out: &mut Vec<GenWrite>,
    seed: u32,
    root: [i32; 3],
    open: impl Fn([i32; 3]) -> bool,
) {
    let origin = ctx.origin_world();
    let mut rng = GenRng::positional(seed, SALT_CEILING, root[0], root[1], root[2]);
    let _ = rng.next_i32(0, 999); // the gather roll
    let mut blocked = false;
    shroom::vine_run(&mut rng, VINE_MAX_LEN, |dy| {
        let cell = [root[0], root[1] + dy, root[2]];
        let taken = Claims::slot(origin, cell).is_some_and(|i| claims.holds(i));
        if blocked || taken || !open(cell) {
            blocked = true;
            return;
        }
        push_if_clear(ctx, claims, out, cell, vine_at(content, seed, cell));
    });
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

/// One lattice cell's cascade outcome, computed once per worker and MEMOIZED.
///
/// The cache is the answer to the purity tax: a 64-block cell overlaps dozens
/// of section dispatches, and every one must know the identical outcome, but
/// deriving it costs a site probe and — for the rare survivor — a domain
/// probe and a containment flood. The memo does not bend the seam contract:
/// the cached value is a pure function of `(seed, cell)` and the positional
/// terrain, so visit order changes only who computes it first, never what it
/// is. Same pattern as `biome_id` below, which caches a pure resolution the
/// same way.
fn cascade_cell(seed: u32, ours: u8, cell: (i32, i32, i32)) -> Option<Rc<cascade::Feature>> {
    /// `(seed, cell x, y, z)` -> the cascade feature that cell resolves to.
    type CascadeCache = std::collections::HashMap<(u32, i32, i32, i32), Option<Rc<cascade::Feature>>>;
    thread_local! {
        static CACHE: std::cell::RefCell<CascadeCache> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let key = (seed, cell.0, cell.1, cell.2);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let out = compute_cascade_cell(seed, ours, cell);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        // The cache only ever grows on cells a streaming session actually
        // visits; the clear is a safety valve for marathon sessions, and
        // clearing changes cost, never content.
        if c.len() > 16384 {
            c.clear();
        }
        c.insert(key, out.clone());
    });
    out
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
    let gate = c.gate_points();
    let want = gate.len();
    let biomes = batched(gate, underground_biome_at);
    if biomes.len() != want || !biomes.contains(&ours) {
        return None;
    }
    let mut coarse: Vec<[i32; 3]> = Vec::new();
    c.coarse_plan(|p| coarse.push(p));
    let want = coarse.len();
    let coarse_solid = batched(coarse, terrain_solid_at);
    if coarse_solid.len() != want {
        return None;
    }
    let traces = c.traces(&coarse_solid);
    if traces.is_empty() {
        return None;
    }
    // The anchors' own biome gate, one small crossing for all tries: a basin
    // heads IN the mushroom cavern, not in whatever cave abuts it.
    let anchors: Vec<[i32; 3]> = traces
        .iter()
        .map(|t| [t.anchor.0, t.s0, t.anchor.1])
        .collect();
    let want = anchors.len();
    let anchor_biomes = batched(anchors, underground_biome_at);
    if anchor_biomes.len() != want {
        return None;
    }
    for (t, &b) in traces.iter().zip(&anchor_biomes) {
        if b != ours {
            continue;
        }
        let mut plan: Vec<[i32; 3]> = Vec::new();
        t.plan(|p| plan.push(p));
        let probe_n = plan.len();
        let replies = batched(plan.clone(), terrain_solid_at);
        if replies.len() != probe_n {
            return None;
        }
        // The plan and this lookup walk the same canonical enumeration, so
        // replies land on the cells that asked for them.
        let lookup: std::collections::HashMap<[i32; 3], bool> =
            plan.into_iter().zip(replies).collect();
        let terrain = |p: [i32; 3]| lookup.get(&p).copied();
        // The cell mutex: the FIRST trace that builds owns the cell — a
        // second accepted trace could overlap the first's footprint, which
        // confinement exists to forbid.
        // Rejection is the exception, not the siting strategy; the pondaudit
        // tooling reports the reasons when they are wanted, so nothing is
        // logged here in normal play.
        let Ok(built) = t.build(&terrain) else {
            continue;
        };
        // Every giant STANDING over the domain, from the same resolver the
        // emit pass uses — root, terrain fit and cap competition already
        // settled, so the containment flood models exactly the bodies the
        // world will hold. The basin then decides who stays out of its water.
        let mut intruders: Vec<cascade::Intruder> = Vec::new();
        for (g, root) in standing_giants_over(seed, ours, built.domain_lo, built.domain_hi) {
            let mut solid = std::collections::BTreeSet::new();
            g.giant.emit(|dx, dy, dz, _| {
                solid.insert([root[0] + dx, root[1] + dy, root[2] + dz]);
            });
            intruders.push(cascade::Intruder {
                key: g.lat,
                root,
                solid,
            });
        }
        return Some(Rc::new(built.finish(&terrain, &intruders)));
    }
    None
}

/// Does a cascade suppress this giant? Cascades are TERRAIN: the basin's
/// containment proof decides which giants stand, and a giant the proof cannot
/// hold with is skipped — in every section, because the answer is a pure
/// function of `(seed, cell)` through the same memo the emitter uses.
fn giant_suppressed(seed: u32, ours: u8, c: &Candidate) -> bool {
    let r = c.giant.reach();
    let lo = [c.x - r, c.cell_floor_y, c.z - r];
    let hi = [c.x + r, c.cell_top_y() + c.giant.rise(), c.z + r];
    for cell in cascade::cells_overlapping_box(lo, hi) {
        if let Some(f) = cascade_cell(seed, ours, cell) {
            if f.suppressed.contains(&c.lat) {
                return true;
            }
        }
    }
    false
}

/// The giant-mushroom roll one lattice cell carries, or `None`. ONE function
/// on purpose: the anchor gather and the cascade's intruder sweep must derive
/// bit-identical mushrooms or the containment flood models a world that is
/// not the one being written.
///
/// Drawn into locals in this order, on purpose: the stream is the world's
/// content, so it must not depend on where a struct literal happens to list
/// its fields.
pub(crate) fn roll_giant(seed: u32, lx: i32, ly: i32, lz: i32) -> Option<Candidate> {
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
pub(crate) fn giant_rolls_over(seed: u32, lo: [i32; 3], hi: [i32; 3]) -> Vec<Candidate> {
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

/// The highest open cell resting on solid, over a column probe whose first
/// slot is the support cell UNDER the window — the root rule every giant
/// stands on, shared so no second derivation can drift from it.
pub(crate) fn highest_floor(solid: &[bool]) -> Option<usize> {
    (1..solid.len()).rev().find(|&k| !solid[k] && solid[k - 1])
}

/// Cap-competition pad: the farthest apart two anchors can sit with their caps
/// still able to interpenetrate — cap radius plus the cap centre's lean offset,
/// for each of the pair. Pinned against rolled maxima by
/// `compete_pad_covers_every_rolled_cap`. The candidate sweep must extend this
/// far past a caller's own margin so a verdict is decided with the whole
/// neighbourhood present, identically from every section.
const COMPETE_PAD: i32 = 20;

/// `(xz, down, up)` reach of a bounded biome gate around the dispatched
/// section: the farthest a cell whose mushroom-biome membership can authorise
/// that pass's write into this section sits from it.
type GatePad = (i32, i32, i32);

/// Giants: a candidate that `reaches_section` roots within a reach horizontally
/// and a rise below, and its roll can sit anywhere inside its anchor lattice
/// cell — so pad by both. Competitors beyond this decide nothing on their own:
/// they can only take a placement AWAY from a candidate, and with no candidate
/// in the biome there is none to take.
const GIANT_PAD: GatePad = (
    MAX_REACH + ANCHOR_LATTICE,
    MAX_RISE + ANCHOR_LATTICE,
    CLAIM_ROWS - 16 + ANCHOR_LATTICE,
);

/// Dressing: a floor plant or curtain root is a cell of this section, except a
/// margin root, which sits in the rows a curtain hangs down through.
const DRESS_PAD: GatePad = (0, 0, CEILING_MARGIN);

/// Can our biome own a cell within `pad` of the dispatched section?
fn ours_in_box(ours: u8, origin: [i32; 3], pad: GatePad) -> bool {
    underground_biomes_in_box(
        [origin[0] - pad.0, origin[1] - pad.1, origin[2] - pad.0],
        [
            origin[0] + 15 + pad.0,
            origin[1] + 15 + pad.2,
            origin[2] + 15 + pad.0,
        ],
    )
    .contains(&ours)
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
pub(crate) fn standing_giants_over(
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
    // Biome gate at the middle of the root window, the same fixed point every
    // other pass asks about.
    let gate: Vec<[i32; 3]> = cands
        .iter()
        .map(|c| [c.x, c.cell_floor_y + ANCHOR_LATTICE / 2, c.z])
        .collect();
    let want = gate.len();
    let biomes = batched(gate, underground_biome_at);
    if biomes.len() != want {
        return Vec::new();
    }
    // Roots for the biome survivors: one column span each, one crossing.
    let mut probe: Vec<[i32; 3]> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        if biomes[i] != ours {
            continue;
        }
        spans.push((i, probe.len()));
        for wy in (c.cell_floor_y - 1)..=c.cell_top_y() {
            probe.push([c.x, wy, c.z]);
        }
    }
    let want = probe.len();
    let solid = batched(probe, terrain_solid_at);
    if solid.len() != want {
        return Vec::new();
    }
    let mut rooted: Vec<(usize, [i32; 3])> = Vec::new();
    for &(i, start) in &spans {
        let c = &cands[i];
        if let Some(k) = highest_floor(&solid[start..start + PROBE_PER_CANDIDATE]) {
            rooted.push((i, [c.x, c.cell_floor_y - 1 + k as i32, c.z]));
        }
    }
    if rooted.is_empty() {
        return Vec::new();
    }
    // The fit skeletons, one more crossing for all of them together.
    let mut probe: Vec<[i32; 3]> = Vec::new();
    let mut fit_at: Vec<usize> = Vec::with_capacity(rooted.len());
    for &(i, root) in &rooted {
        fit_at.push(probe.len());
        cands[i].giant.fit_probes(|dx, dy, dz| {
            probe.push([root[0] + dx, root[1] + dy, root[2] + dz]);
        });
    }
    let want = probe.len();
    let solid = batched(probe, terrain_solid_at);
    if solid.len() != want {
        return Vec::new();
    }
    let mut viable: Vec<(usize, [i32; 3])> = Vec::new();
    for (k, &(i, root)) in rooted.iter().enumerate() {
        let end = fit_at.get(k + 1).copied().unwrap_or(solid.len());
        if solid[fit_at[k]..end].iter().all(|&s| !s) {
            viable.push((i, root));
        }
    }
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
fn beats(a: &Candidate, ra: [i32; 3], b: &Candidate, rb: [i32; 3]) -> bool {
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

/// Run one batched host query, split into ABI-sized pieces.
///
/// Every real section fits in a single crossing — the split exists so a
/// pathological one degrades into a second call instead of having the host
/// REJECT the batch, which would stop the pack generating anything at all, or
/// having a cap TRUNCATE it, which drops candidates by list position and lets a
/// cell that is about to lose to a giant displace a legitimate floor.
fn batched<T>(positions: Vec<[i32; 3]>, call: fn(Vec<[i32; 3]>) -> Vec<T>) -> Vec<T> {
    if positions.is_empty() {
        return Vec::new();
    }
    if positions.len() <= ABI_BATCH_MAX {
        return call(positions);
    }
    let mut out = Vec::with_capacity(positions.len());
    for chunk in positions.chunks(ABI_BATCH_MAX) {
        out.extend(call(chunk.to_vec()));
    }
    out
}

/// The vine segment at ONE cell of a curtain: plain, or the flowering variant
/// of the stand it hangs in.
///
/// Derived from the cell's world position and nothing else — not the root, not
/// the run's length, not how far down the run the cell sits, and not a draw off
/// the root's stream. A curtain is gathered from the DISPATCHING section's own
/// snapshot, so any of those would let two sections disagree about one cell;
/// keyed on position, every section that can see the cell derives the same
/// segment. `dy` in particular would bloom every curtain at the same height.
fn vine_at(content: &Content, seed: u32, cell: [i32; 3]) -> BlockId {
    let mut rng = GenRng::positional(seed, SALT_VINE_BLOOM, cell[0], cell[1], cell[2]);
    if rng.next_i32(0, 999) < VINE_BLOOM_PER_MILLE {
        pick_species(content, seed, cell[0], cell[1], cell[2]).glow_vine
    } else {
        content.vine
    }
}

/// Which species owns a spot. Derived positionally on a COARSE grid so a cavern
/// reads as stands of one colour rather than confetti — the single decision
/// that most affects how the place feels.
/// Ground flora grows in COLONIES, not as an even dusting. A colony is a
/// rolled centre on a column lattice with a radius; density falls off from its
/// middle, so a patch has a dense heart and a ragged edge — spores landing and
/// spreading, rather than a per-cell dice roll over the whole floor.
///
/// A COLUMN lattice, deliberately: a patch is a thing on the ground, so it
/// spans a footprint and not a volume. A cavern with two floor levels in one
/// column grows the same colony on both, which is what falling spores would do
/// anyway.
///
/// Pure in `(seed, column)`, so the gather pre-roll and the emit pass compute
/// the identical density and can never disagree about which cells to keep —
/// they are the same function, not two implementations of one rule.
const PATCH_LATTICE: i32 = 11;
/// One in N lattice cells seeds a colony.
const PATCH_ONE_IN: i32 = 2;
/// Colony radius in blocks, rolled per patch.
const PATCH_R: (i32, i32) = (3, 7);
/// Per-mille density at a colony's centre, and out at its rim.
const PATCH_CORE_PER_MILLE: i32 = 340;
const PATCH_RIM_PER_MILLE: i32 = 40;
/// Per-mille density with no colony overhead — the odd straggler, so the floor
/// between patches is not sterile.
const STRAY_PER_MILLE: i32 = 3;
/// Share of a colony that is its MINORITY kind, per mille of the patch's own
/// cells. A pure stand reads stamped; a few of the other kind reads seeded.
const PATCH_ADMIX_PER_MILLE: i32 = 120;

/// What grows at a column, and how densely: `(per-mille, kind_is_flower,
/// species_salt)`. The species salt is the COLONY's, not the cell's, so one
/// patch is one colour — the thing that makes it read as a single organism's
/// spread rather than confetti.
fn patch_at(seed: u32, wx: i32, wz: i32) -> (i32, bool, i32) {
    let (mut best, mut flower, mut salt) = (STRAY_PER_MILLE, false, 0);
    let cell = |v: i32| v.div_euclid(PATCH_LATTICE);
    // A colony reaches at most PATCH_R.1 blocks, so only the lattice cells
    // within that of this column can own us.
    for lz in cell(wz - PATCH_R.1)..=cell(wz + PATCH_R.1) {
        for lx in cell(wx - PATCH_R.1)..=cell(wx + PATCH_R.1) {
            let mut rng = GenRng::positional(seed, SALT_PATCH, lx, 0, lz);
            if rng.next_i32(0, PATCH_ONE_IN - 1) != 0 {
                continue;
            }
            let cx = lx * PATCH_LATTICE + rng.next_i32(0, PATCH_LATTICE - 1);
            let cz = lz * PATCH_LATTICE + rng.next_i32(0, PATCH_LATTICE - 1);
            let r = rng.next_i32(PATCH_R.0, PATCH_R.1);
            let is_flower = rng.next_i32(0, 1) == 1;
            let sp = rng.next_i32(0, 1 << 20);
            let (dx, dz) = (wx - cx, wz - cz);
            let d2 = dx * dx + dz * dz;
            if d2 > r * r {
                continue;
            }
            // Linear in DISTANCE, not in distance squared: squared falls off
            // far too slowly near the centre and gives a flat-topped disc.
            let d = isqrt(d2);
            let dens =
                PATCH_CORE_PER_MILLE + (PATCH_RIM_PER_MILLE - PATCH_CORE_PER_MILLE) * d / r.max(1);
            // Overlapping colonies do not stack — the densest simply owns the
            // cell, so two patches meeting read as two patches, not a bloom.
            if dens > best {
                best = dens;
                flower = is_flower;
                salt = sp;
            }
        }
    }
    (best, flower, salt)
}

/// Integer square root, for the falloff. `f64::sqrt` would be fine here, but
/// every other shape decision in this pack is integer arithmetic and mixing
/// the two invites a rounding difference between two derivations of one cell.
fn isqrt(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// The species a COLONY wears. Falls back to the ambient stand colour for a
/// stray growing outside any patch, so the two blend instead of the strays
/// advertising themselves as different.
fn patch_species(content: &Content, seed: u32, salt: i32, p: [i32; 3]) -> &Species {
    if salt == 0 {
        return pick_species(content, seed, p[0], p[1], p[2]);
    }
    &content.species[(salt as usize) % content.species.len()]
}

fn pick_species(content: &Content, seed: u32, wx: i32, wy: i32, wz: i32) -> &Species {
    // 24 put one species in an entire view, which reads monochrome. 13 lets two
    // or three stands share a sightline, so the per-channel light MIXING is
    // visible — a blue stand behind a pink one is the whole point of doing
    // coloured light properly rather than tinting.
    const STAND: i32 = 13;
    let mut rng = GenRng::positional(
        seed,
        SALT_SPECIES,
        wx.div_euclid(STAND),
        wy.div_euclid(STAND),
        wz.div_euclid(STAND),
    );
    let i = rng.next_i32(0, content.species.len() as i32 - 1) as usize;
    &content.species[i]
}

/// This pack's underground-biome id. Worldgen runs on detached instances, so
/// this is resolved per instance and cached rather than held in mod state.
fn biome_id() -> Option<u8> {
    thread_local! {
        static ID: Option<u8> = resolve_underground_biome(crate::BIOME_KEY);
    }
    ID.with(|id| *id)
}

fn is_open(ctx: &GenCtx, p: [i32; 3]) -> bool {
    matches!(ctx.block(p), Some(b) if b.0 == 0)
}

/// Is the cell `dy` from `p` solid? `None` when it lies outside the dispatching
/// section, where `GenCtx` knows nothing and only the positional terrain can
/// answer. Reading `None` as "not solid" is what left the section planes bare.
fn neighbour_solid(ctx: &GenCtx, p: [i32; 3], dy: i32) -> Option<bool> {
    ctx.block([p[0], p[1] + dy, p[2]]).map(|b| b.0 != 0)
}

/// Rows the claim set spans: this section's own sixteen, plus the margin rows
/// over its roof.
const CLAIM_ROWS: i32 = 16 + CEILING_MARGIN;

/// Cells this dispatch has spoken for.
///
/// STRUCTURE OUTRANKS DRESSING. A giant's stem base stands on the highest open
/// cell resting on rock, which is exactly what a floor flower asks for, so
/// without a record of what is already taken both are emitted for that cell and
/// whichever write the engine applies last wins — a cave flower growing inside
/// a mushroom stem. The giants are emitted first and everything after them
/// consults this.
///
/// It reaches `CEILING_MARGIN` rows ABOVE the section, which are not ours to
/// write. A curtain hanging in from up there walks through those rows, and a
/// mushroom standing in one stops the run — in the section that owns the root
/// as well as here, because a giant is a pure function of its anchor and both
/// sections reconstruct it. Without that the two disagree about where the run
/// ends and the curtain comes out with nothing solid over its top cell, which
/// is exactly the support the vine rows are declared with.
struct Claims([u64; (256 * CLAIM_ROWS as usize).div_ceil(64)]);

impl Default for Claims {
    fn default() -> Claims {
        Claims([0; (256 * CLAIM_ROWS as usize).div_ceil(64)])
    }
}

impl Claims {
    /// This cell's index, or `None` when it is out of the claim window
    /// entirely. Being IN the window does not make a cell writable — see
    /// [`push_if_clear`].
    fn slot(origin: [i32; 3], p: [i32; 3]) -> Option<usize> {
        let (lx, ly, lz) = (p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]);
        let inside =
            (0..16).contains(&lx) && (0..CLAIM_ROWS).contains(&ly) && (0..16).contains(&lz);
        inside.then(|| (ly * 256 + lz * 16 + lx) as usize)
    }

    fn holds(&self, i: usize) -> bool {
        self.0[i / 64] & (1u64 << (i % 64)) != 0
    }

    /// Take `i`, or answer `false` when it is already spoken for.
    fn take(&mut self, i: usize) -> bool {
        let free = !self.holds(i);
        self.0[i / 64] |= 1u64 << (i % 64);
        free
    }
}

/// Write into a cell this section owns, that the snapshot says is open, and
/// that nothing earlier in this dispatch has already claimed.
///
/// A cell in the margin rows is CLAIMED but never written: the section above
/// owns it and writes its own copy. Claiming it is what makes a curtain stop at
/// the same mushroom on both sides of the plane.
fn push_if_clear(
    ctx: &GenCtx,
    claims: &mut Claims,
    out: &mut Vec<GenWrite>,
    p: [i32; 3],
    block: BlockId,
) {
    let Some(i) = Claims::slot(ctx.origin_world(), p) else {
        return;
    };
    let ours = ctx.block(p).is_some();
    if ours && !is_open(ctx, p) {
        return;
    }
    if claims.take(i) && ours {
        out.push((p, block));
    }
}

/// Write into a cell this section owns that nothing earlier in the dispatch has
/// claimed — WITHOUT the openness test.
///
/// A cascade is cut out of rock, so "is the snapshot open here" is the wrong
/// question for it: every cell it replaces was proven SOLID by the same
/// positional terrain read the containment proof is built on, and a cell that
/// read open there would mean the proof was taken against different terrain
/// from the one being written. Refusing on openness would punch holes in a
/// gorge wall exactly where the water is.
fn push_over_terrain(
    ctx: &GenCtx,
    claims: &mut Claims,
    out: &mut Vec<GenWrite>,
    p: [i32; 3],
    block: BlockId,
) {
    let Some(i) = Claims::slot(ctx.origin_world(), p) else {
        return;
    };
    if claims.take(i) && ctx.block(p).is_some() {
        out.push((p, block));
    }
}

/// Speak for a cell without writing it. A cascade uses this twice: for the
/// cavern cells over its own pools, so no flower or sporeshroom is dressed
/// standing on water (their support is the cell below, which is about to become
/// a fluid), and for the rock of its own wall, which must stay exactly as the
/// carver left it.
fn reserve(ctx: &GenCtx, claims: &mut Claims, p: [i32; 3]) {
    if let Some(i) = Claims::slot(ctx.origin_world(), p) {
        claims.take(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ground flora must CLUMP. The whole point of the colony lattice is that
    /// the floor is not an even dusting, and "it clumps" is exactly the kind of
    /// property a later tuning edit silently destroys — drop the falloff and
    /// every number below still looks plausible while the caverns go back to
    /// confetti. Measured three ways over a real column sweep:
    ///
    /// - the densest columns are far denser than the sparsest (a flat roll
    ///   gives one number everywhere);
    /// - a good share of the floor is near-empty, so patches read as patches;
    /// - a cell in a colony overwhelmingly shares its neighbour's species,
    ///   which is what makes a stand look like one organism's spread.
    #[test]
    fn ground_flora_grows_in_colonies_not_an_even_dusting() {
        for seed in [0x312u32, 0x1D001, 0x2BEEF] {
            let mut dens: Vec<i32> = Vec::new();
            let (mut same, mut pairs) = (0usize, 0usize);
            for wz in -60..60 {
                for wx in -60..60 {
                    let (d, _, salt) = patch_at(seed, wx, wz);
                    dens.push(d);
                    // Species agreement with the neighbour to the east, over
                    // cells that are both inside some colony.
                    let (dn, _, sn) = patch_at(seed, wx + 1, wz);
                    if salt != 0 && sn != 0 && d > STRAY_PER_MILLE && dn > STRAY_PER_MILLE {
                        pairs += 1;
                        same += (salt % 4 == sn % 4) as usize;
                    }
                }
            }
            dens.sort_unstable();
            let p10 = dens[dens.len() / 10];
            let p95 = dens[dens.len() * 95 / 100];
            let bare = dens.iter().filter(|&&d| d <= STRAY_PER_MILLE).count();
            assert!(
                p95 >= p10 * 4,
                "flora is nearly uniform (p10 {p10}, p95 {p95}) — the colonies are gone \
                 (seed {seed:#x})"
            );
            assert!(
                bare * 5 >= dens.len(),
                "only {bare}/{} columns are bare; patches need gaps between them \
                 (seed {seed:#x})",
                dens.len()
            );
            assert!(
                pairs > 200 && same * 10 >= pairs * 8,
                "neighbouring colony cells agree on species only {same}/{pairs} of the time \
                 (seed {seed:#x})"
            );
        }
    }

    /// The margin this feature scans with MUST cover the largest mushroom it
    /// can roll, in both axes. If a roll can reach further than `MAX_REACH` /
    /// `MAX_RISE`, a neighbouring section never iterates that anchor and the
    /// mushroom comes out sliced at the boundary — a corruption that looks like
    /// a rendering bug and is miserable to trace back to here.
    #[test]
    fn rolled_mushrooms_never_escape_the_scan_margin() {
        for i in 0..4000i32 {
            let mut rng = GenRng::positional(0xC0FFEE, SALT_GIANT, i, i * 7, i * 13);
            let scale = rng.next_i32(0, 255) as u8;
            let g = Giant::roll(&mut rng, scale);
            assert!(
                g.reach() <= MAX_REACH,
                "reach {} exceeds MAX_REACH {MAX_REACH} for {g:?}",
                g.reach()
            );
            let mut top = 0;
            g.emit(|_, dy, _, _| top = top.max(dy));
            assert!(
                top <= MAX_RISE,
                "rise {top} exceeds MAX_RISE {MAX_RISE} for {g:?}"
            );
        }
    }

    /// COMPETE_PAD must cover the farthest two anchors whose caps can still
    /// interpenetrate — cap radius plus lean offset, for each of the pair. If
    /// a roll can compete from further away, a section resolves the contest
    /// with an incomplete neighbourhood and disagrees with its neighbour, and
    /// the mushroom comes out standing in one section and absent in the next.
    #[test]
    fn compete_pad_covers_every_rolled_cap() {
        let mut worst = 0;
        for i in 0..4000i32 {
            let mut rng = GenRng::positional(0xC0FFEE, SALT_GIANT, i, i * 7, i * 13);
            let scale = rng.next_i32(0, 255) as u8;
            let g = Giant::roll(&mut rng, scale);
            let (cx, cz, r) = g.cap_footprint();
            worst = worst.max(cx.abs().max(cz.abs()) + r);
        }
        assert!(
            2 * worst <= COMPETE_PAD,
            "two rolled caps can compete from {} apart, past COMPETE_PAD {COMPETE_PAD}",
            2 * worst
        );
    }

    /// Of two viable mushrooms whose caps interpenetrate, exactly the smaller
    /// anchor stands; caps that merely touch both stand. The verdict must be
    /// symmetric — `beats` one way implies not-`beats` the other — or two
    /// sections could each keep their own local favourite.
    #[test]
    fn cap_competition_is_deterministic_and_one_sided() {
        let giant = |cap_r: i32| Giant {
            form: shroom::Form::Flatcap,
            height: 9,
            stem_r: 1,
            cap_r,
            skirt: 1,
            lean_x: 0,
            lean_z: 0,
        };
        let cand = |lat: [i32; 3], x: i32, z: i32, cap_r: i32| Candidate {
            x,
            z,
            cell_floor_y: lat[1] * ANCHOR_LATTICE,
            lat,
            giant: giant(cap_r),
        };
        let a = cand([0, 0, 0], 0, 0, 5);
        let b = cand([1, 0, 0], 8, 0, 5);
        let (ra, rb) = ([0, -30, 0], [8, -30, 0]);
        // 8 apart with radii 5+5: interpenetrating. Lower lat wins, one-sided.
        assert!(beats(&a, ra, &b, rb));
        assert!(!beats(&b, rb, &a, ra));
        // Same geometry at exactly the radius sum: touching, both stand.
        let c = cand([1, 0, 0], 10, 0, 5);
        assert!(!beats(&a, ra, &c, [10, -30, 0]));
        // Vertically separated caps never compete however close in plan.
        assert!(!beats(&a, ra, &b, [8, 30, 0]));
    }

    /// One dispatch's terrain probe is SPLIT at the ABI cap rather than
    /// truncated, so overflowing it can no longer make the host reject the call
    /// and stop giants generating everywhere. What is still worth pinning is
    /// the SIZE: every knob below is a tuning knob, and a change that turned
    /// one dispatch into a dozen crossings would be a watchdog problem long
    /// before it was a correctness one.
    #[test]
    fn one_dispatch_stays_within_a_couple_of_abi_batches() {
        // Worst case: every lattice cell in the scan window rolls a candidate,
        // and every candidate probes its whole cell plus the support cell below.
        let cells = |span: i32| (span.div_euclid(ANCHOR_LATTICE) + 2) as usize;
        let giants = cells(16 + 2 * MAX_REACH).pow(2) * cells(16 + MAX_RISE) * PROBE_PER_CANDIDATE;
        // Every cell of the two boundary rows needs its unseen neighbour, and
        // every column can carry one margin probe block.
        let dressing = 2 * 256 + 256 * PROBE_PER_MARGIN;
        let worst = giants + dressing;
        assert!(
            worst <= 2 * ABI_BATCH_MAX,
            "worst-case probe batch {worst} ({giants} giants, {dressing} dressing) \
             needs more than two ABI batches"
        );
    }

    /// A dispatch consults the cascade cache once per overlapping lattice
    /// cell. The count is what a retune of the cascade lattice would
    /// multiply — each MISS costs a site probe and, for a survivor, a domain
    /// probe (`cascade::tests::probes_stay_within_budget` bounds those).
    #[test]
    fn a_dispatch_consults_only_a_handful_of_cascade_cells() {
        for origin in [[0, 0, 0], [16, -48, -16], [-16, -64, 48]] {
            let cells = cascade::cells_overlapping(origin, CLAIM_ROWS).len();
            assert!(cells <= 12, "a dispatch consults {cells} cascade cells");
        }
    }

    /// A vine curtain hangs DOWN, so it routinely crosses the floor of the
    /// section its root sits in. Every cell of a run must therefore be reachable
    /// by the section that OWNS that cell — either because the root is inside
    /// it, or because the root falls in the margin rows it scans over its own
    /// roof. Miss that and curtains end on the `y % 16 == 0` planes, which reads
    /// as a short vine rather than as the seam bug it is.
    #[test]
    fn every_cell_of_a_curtain_is_reachable_by_the_section_that_owns_it() {
        for root in -40..40i32 {
            for len in 1..=VINE_MAX_LEN {
                for d in 0..len {
                    let cell = root - d;
                    let origin = cell.div_euclid(16) * 16;
                    let ly = root - origin;
                    assert!(
                        (0..16 + CEILING_MARGIN).contains(&ly),
                        "root {root} writes {cell}, but the section at {origin} \
                         that owns that cell never scans row {ly}"
                    );
                }
            }
        }
    }

    /// The margin must not be WIDER than curtains reach either: a row whose run
    /// cannot touch this section is a positional roll and up to seven terrain
    /// probes, paid per column per section.
    #[test]
    fn the_ceiling_margin_is_no_wider_than_a_curtain_reaches() {
        // the highest row a run of maximum length can be rooted on and still
        // put its last cell in row 15
        assert_eq!(CEILING_MARGIN, VINE_MAX_LEN - 1);
        assert_eq!(15 + VINE_MAX_LEN - 1, 15 + CEILING_MARGIN);
    }
}

#[cfg(test)]
mod ctx_tests {
    use super::*;

    /// Solid rock below the section's mid-line, open air above — so the
    /// occupancy predicates have a real floor plane and ceiling plane to find.
    fn split_section(section: [i32; 3]) -> GenCtx {
        let mut blocks = vec![0u8; 4096];
        for ly in 0..8 {
            for lz in 0..16 {
                for lx in 0..16 {
                    blocks[ly * 256 + lz * 16 + lx] = 3;
                }
            }
        }
        GenCtx::for_test(section, 0xC0FFEE, blocks, vec![64; 256], vec![0; 256], 62)
    }

    /// A section of nothing but air: both boundary rows have an unseen
    /// neighbour, which is the case the probes exist for.
    fn open_section(section: [i32; 3]) -> GenCtx {
        GenCtx::for_test(
            section,
            0xC0FFEE,
            vec![0u8; 4096],
            vec![64; 256],
            vec![0; 256],
            62,
        )
    }

    /// Floor flora reads the cell UNDER the candidate, and on local row 0 that
    /// cell belongs to the section beneath. Answering "not solid" there is what
    /// left every `y % 16 == 0` plane — the world floor at the bottom of the
    /// biome band above all, the widest flat floor in the cavern — with no
    /// flora at all while the giants standing on it generated normally.
    #[test]
    fn the_bottom_row_asks_the_terrain_for_the_support_it_cannot_see() {
        let ctx = open_section([0, -3, 0]);
        let origin = ctx.origin_world();
        let (floors, ceilings, _, _) = gather_dressing(&ctx, ctx.seed());
        let bottom: Vec<&Dress> = floors.iter().filter(|d| d.p[1] == origin[1]).collect();
        assert!(
            !bottom.is_empty(),
            "no candidate rolled on the bottom row; the test proves nothing"
        );
        for d in bottom {
            assert_eq!(d.below, None, "row 0 cannot see its own support");
            assert_eq!(
                d.unseen(),
                Some([d.p[0], origin[1] - 1, d.p[2]]),
                "the probe must be the support cell, not the roof"
            );
        }
        let top: Vec<&Dress> = ceilings
            .iter()
            .filter(|d| d.p[1] == origin[1] + 15)
            .collect();
        assert!(!top.is_empty(), "no candidate rolled on the top row");
        for d in top {
            assert_eq!(
                d.unseen(),
                Some([d.p[0], origin[1] + 16, d.p[2]]),
                "row 15 must probe the roof it cannot see"
            );
        }
    }

    /// Inside the section the snapshot is authoritative and no probe is spent:
    /// the cell below a mid-column candidate is one this section owns, so
    /// asking the host about it would be a crossing bought for nothing.
    #[test]
    fn a_candidate_that_can_see_both_neighbours_costs_no_probe() {
        let ctx = split_section([0, -3, 0]);
        let (floors, ceilings, _, _) = gather_dressing(&ctx, ctx.seed());
        let mut inner = 0;
        for d in floors.iter().chain(&ceilings) {
            let ly = d.p[1] - ctx.origin_world()[1];
            if (1..15).contains(&ly) {
                inner += 1;
                assert_eq!(d.unseen(), None, "row {ly} sees both its neighbours");
            }
        }
        assert!(inner > 0, "no interior candidate rolled");
        // rock fills rows 0..8, so every floor candidate rests on that plane
        for d in &floors {
            assert_eq!(d.below, Some(true), "a floor candidate rests on rock");
        }
    }

    /// A curtain rooted over our roof must be re-derived HERE, because the
    /// section that owns the root cannot write into us. Roots are scanned in
    /// the margin rows and only in columns whose top row is open, which is
    /// exactly the set of columns a curtain can reach us through.
    #[test]
    fn roots_above_the_roof_are_scanned_when_a_curtain_can_reach_in() {
        let ctx = open_section([0, -3, 0]);
        let origin = ctx.origin_world();
        let (_, _, margins, cols) = gather_dressing(&ctx, ctx.seed());
        assert!(
            !margins.is_empty(),
            "no margin root rolled over an open roof"
        );
        for m in &margins {
            let ly = m.p[1] - origin[1];
            assert!(
                (16..16 + CEILING_MARGIN).contains(&ly),
                "margin root at row {ly} is outside the scanned band"
            );
            let c = &cols[m.col];
            assert_eq!(c.xz, [m.p[0], m.p[2]], "root filed under the wrong column");
            assert!(
                c.rows >= (ly - 16) as usize + 2 && c.rows <= PROBE_PER_MARGIN,
                "a root on row {ly} reads past its column's {} probed rows",
                c.rows
            );
        }
        // a section with a sealed roof pays nothing for the margin scan
        let sealed = split_section([0, -3, 0]);
        let mut blocks = vec![3u8; 4096];
        for i in 0..256 {
            blocks[i] = 0; // one open row at the bottom, roof solid
        }
        let sealed = GenCtx::for_test(
            sealed.section_pos(),
            sealed.seed(),
            blocks,
            vec![64; 256],
            vec![0; 256],
            62,
        );
        let (_, _, margins, _) = gather_dressing(&sealed, sealed.seed());
        assert!(margins.is_empty(), "a sealed roof cannot admit a curtain");
    }

    fn test_content() -> Content {
        Content {
            stem: BlockId(200),
            vine: BlockId(202),
            water: BlockId(203),
            silt: BlockId(204),
            air: BlockId(0),
            species: (0..4)
                .map(|i| Species {
                    cap: BlockId(210 + i),
                    sporeshroom: BlockId(220 + i),
                    flower: BlockId(230 + i),
                    glow_vine: BlockId(240 + i),
                })
                .collect(),
        }
    }

    /// A DRESSING block may never replace a STRUCTURAL one.
    ///
    /// The two passes want the same cell by construction — a stem's base is the
    /// highest open cell resting on rock, which is the definition of a floor
    /// candidate — and the snapshot cannot separate them, because the giant's
    /// cells are still pending in this dispatch's own write list. Rachel found
    /// this as a cave flower growing inside a mushroom stem.
    #[test]
    fn a_dressing_block_never_replaces_a_structural_block() {
        let ctx = open_section([0, -3, 0]);
        let content = test_content();
        let mut claims = Claims::default();
        let mut out: Vec<GenWrite> = Vec::new();

        let mut rng = GenRng::positional(ctx.seed(), SALT_GIANT, 1, 2, 3);
        let giant = Giant::roll(&mut rng, 200);
        let root = [
            ctx.origin_world()[0] + 8,
            ctx.origin_world()[1],
            ctx.origin_world()[2] + 8,
        ];
        giant.emit(|dx, dy, dz, part| {
            let block = match part {
                Part::Stem => content.stem,
                Part::Cap | Part::Gill => content.species[0].cap,
            };
            push_if_clear(
                &ctx,
                &mut claims,
                &mut out,
                [root[0] + dx, root[1] + dy, root[2] + dz],
                block,
            );
        });
        let structural = out.len();
        assert!(structural > 0, "the giant emitted nothing into the section");

        // now let every dressing pass in the module try to take those cells
        for &(p, _) in &out.clone() {
            push_if_clear(&ctx, &mut claims, &mut out, p, content.species[1].flower);
            push_if_clear(&ctx, &mut claims, &mut out, p, content.vine);
        }
        assert_eq!(
            out.len(),
            structural,
            "a dressing write took a structural cell"
        );

        // and every structural cell still holds its own block
        let mut seen = std::collections::HashSet::new();
        for &(p, b) in &out {
            assert!(seen.insert(p), "the same cell was written twice: {p:?}");
            assert!(
                b == content.stem || b == content.species[0].cap,
                "cell {p:?} holds {b:?}, not the mushroom's own block"
            );
        }
    }

    /// Species are picked on a coarse grid so a cavern reads as STANDS of one
    /// colour. If this ever became per-cell the place would look like confetti.
    #[test]
    fn species_are_stable_across_a_stand_and_vary_between_stands() {
        let content = test_content();
        let at = |x, z| pick_species(&content, 7, x, -40, z).cap.0;
        let base = at(0, 0);
        for d in 0..8 {
            assert_eq!(at(d, d), base, "species changed inside one stand");
        }
        let far: Vec<u8> = (1..12).map(|k| at(k * 40, k * 40)).collect();
        assert!(
            far.iter().any(|&c| c != base),
            "species never varies between distant stands: {far:?}"
        );
    }

    /// A curtain picks each segment from that CELL's world position alone.
    ///
    /// The run is gathered from the DISPATCHING section's own snapshot, so a
    /// choice drawn off the root's rng stream — or off the offset down the run
    /// — comes out differently depending on which section wrote the cell and on
    /// how long that particular curtain happened to roll. Two curtains that
    /// overlap must agree on every shared cell, and the bloom must wear the
    /// colour of the stand the CELL is in, not the one the root is in.
    #[test]
    fn a_curtain_picks_each_segment_from_that_cell_alone() {
        let content = test_content();
        let seed = 0x1D001;
        let run = |root: i32, len: i32| -> Vec<u8> {
            // A stream the choice must not read: if the bloom ever comes off
            // the root's draws again, the two runs diverge here.
            let mut stream = GenRng::positional(seed, SALT_CEILING, 4, root, -9);
            (0..len)
                .map(|d| {
                    let _ = stream.next_i32(0, 999);
                    vine_at(&content, seed, [4, root - d, -9]).0
                })
                .collect()
        };
        assert_eq!(
            run(-30, 7)[3..],
            run(-33, 4)[..],
            "two curtains disagree about the cells they share"
        );

        let curtains: Vec<Vec<[i32; 3]>> = (0..40)
            .flat_map(|x| (0..40).map(move |z| (x, z)))
            .map(|(x, z)| (0..VINE_MAX_LEN).map(|d| [x, -34 - d, z]).collect())
            .collect();
        let mut cells = 0usize;
        let mut blooms = 0usize;
        let mut mixed = 0usize;
        for curtain in &curtains {
            let run: Vec<BlockId> = curtain
                .iter()
                .map(|&c| vine_at(&content, seed, c))
                .collect();
            for (&cell, &block) in curtain.iter().zip(&run) {
                assert!(
                    block == content.vine
                        || block
                            == pick_species(&content, seed, cell[0], cell[1], cell[2]).glow_vine,
                    "the bloom at {cell:?} is not the colour of the stand it hangs in"
                );
            }
            cells += run.len();
            blooms += run.iter().filter(|&&b| b != content.vine).count();
            if run.iter().any(|&b| b == content.vine) && run.iter().any(|&b| b != content.vine) {
                mixed += 1;
            }
        }
        let share = blooms as f64 / cells as f64;
        assert!(
            (0.02..0.25).contains(&share),
            "blooms are an accent on a plain strand, not absent and not the \
             norm: {share:.3} of {cells} cells"
        );
        // Keyed on anything a whole column shares — the root, the run, x/z
        // alone — curtains go back to all-plain-or-all-glowing, which is the
        // look this replaced.
        assert!(
            mixed * 4 > curtains.len(),
            "only {mixed} of {} curtains mix plain and flowering segments",
            curtains.len()
        );
    }

    /// The engine dispatches this feature for EVERY section, so the pack owns
    /// its own altitude gate. A section clear of the band must cost nothing —
    /// and the gate has to fire before the first host call, which is what this
    /// asserts: off wasm a host call panics, so a regressed gate fails loudly
    /// here rather than quietly burning a lattice sweep per section in flight.
    #[test]
    fn a_section_above_the_biome_band_does_no_work_at_all() {
        let content = test_content();
        let first_clear = TOP_CONTENT_Y.div_euclid(16) + 1;
        let ctx = split_section([0, first_clear, 0]);
        assert!(generate(&content, &ctx).is_empty());
    }
}
