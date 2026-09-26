//! Cavern dressing: floor flora grown in colonies, and vine curtains hung
//! from the roof — including roots over this section's roof whose curtains
//! hang down into it.
//!
//! A decoration is a single cell owned by exactly one section, so it needs no
//! cross-section agreement — but it does need the truth about its own support
//! and roof, and on local rows 0 and 15 those lie in the NEIGHBOURING section.
//! Treating an unseen neighbour as "not solid" left every `y % 16 == 0` plane
//! undressed, the world floor at the bottom of the biome band worst of all.
//! Every read that reaches outside the section is therefore positional
//! ([`TerrainReads`]); the ones that test a cell this section owns stay on
//! `GenCtx::block`, where disagreement is impossible by construction.

use mod_sdk::*;

use super::claims::Emitter;
use super::{
    is_open, neighbour_rock, pick_species, CEILING_MARGIN, SALT_CEILING, SALT_GROUND, SALT_PATCH,
    SALT_VINE_BLOOM, VINE_MAX_LEN,
};
use crate::content::{Content, Species};
use crate::probe::{self, Query, TerrainReads};
use crate::shroom;

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
pub(super) const STRAY_PER_MILLE: i32 = 3;
/// The flora colonies, over the constants above.
const PATCHES: ColonyField = ColonyField {
    salt: SALT_PATCH,
    lattice: PATCH_LATTICE,
    one_in: PATCH_ONE_IN,
    radius: PATCH_R,
    core: PATCH_CORE_PER_MILLE,
    rim: PATCH_RIM_PER_MILLE,
    stray: STRAY_PER_MILLE,
};
/// Share of a colony that is its MINORITY kind, per mille of the patch's own
/// cells. A pure stand reads stamped; a few of the other kind reads seeded.
const PATCH_ADMIX_PER_MILLE: i32 = 120;

/// One rolled decoration cell inside this section, with whatever `GenCtx` could
/// tell us about the two cells that classify it.
///
/// `None` means the neighbour lies in the section above or below. That happens
/// on exactly two rows — local 0 has no readable support, local 15 no readable
/// roof — and it is resolved by a positional probe, never by assuming the
/// neighbour is air.
pub(super) struct Dress {
    pub(super) p: [i32; 3],
    pub(super) below: Option<bool>,
    pub(super) above: Option<bool>,
}

impl Dress {
    /// The one neighbour this cell cannot classify itself from. A 16-tall
    /// section can never lack both.
    pub(super) fn unseen(&self) -> Option<[i32; 3]> {
        match (self.below, self.above) {
            (None, _) => Some([self.p[0], self.p[1] - 1, self.p[2]]),
            (_, None) => Some([self.p[0], self.p[1] + 1, self.p[2]]),
            _ => None,
        }
    }

    /// Resting on rock, once the probe (if any) has answered.
    fn on_rock(&self, reads: &TerrainReads) -> bool {
        self.below
            .unwrap_or_else(|| reads.rock([self.p[0], self.p[1] - 1, self.p[2]]))
    }

    /// Under a roof, same.
    fn under_roof(&self, reads: &TerrainReads) -> bool {
        self.above
            .unwrap_or_else(|| reads.rock([self.p[0], self.p[1] + 1, self.p[2]]))
    }
}

/// A vine root sitting ABOVE this section's roof, and the column probe block
/// that answers for it. The section that owns the root cannot write our cells,
/// so a curtain only crosses the plane because we re-derive its root here.
pub(super) struct Margin {
    pub(super) p: [i32; 3],
    /// Index into `margin_cols` — roots in one column share one probe block.
    pub(super) col: usize,
}

/// A column of terrain cells over this section's roof, and how many of them the
/// roots filed against it actually read: rows `16 ..= 16 + k + 1` for the
/// highest root at `16 + k`. Probing the whole margin band for every column
/// would be `PROBE_PER_MARGIN` point queries where two usually suffice.
pub(super) struct MarginCol {
    pub(super) xz: [i32; 2],
    pub(super) rows: usize,
}

/// Every rolled decoration cell this section owns, plus the vine roots
/// sitting over its roof and the columns those roots probe.
#[derive(Default)]
pub(super) struct Dressing {
    pub(super) floors: Vec<Dress>,
    pub(super) ceilings: Vec<Dress>,
    pub(super) margins: Vec<Margin>,
    pub(super) margin_cols: Vec<MarginCol>,
}

/// The dressing after its two crossings: which candidates the biome kept
/// (parallel to floors, then ceilings, then margins), and the positional
/// answers for every neighbour they could not see.
pub(super) struct Resolved {
    dressing: Dressing,
    mine: Vec<bool>,
    reads: TerrainReads,
}

impl Dressing {
    /// Gather every candidate. Pure: no host calls, so the roll filters the
    /// candidate list down before anything crosses the ABI.
    pub(super) fn gather(content: &Content, ctx: &GenCtx, seed: u32) -> Dressing {
        let origin = ctx.origin_world();
        let mut d = Dressing::default();
        for lz in 0..16 {
            for lx in 0..16 {
                let (x, z) = (origin[0] + lx, origin[2] + lz);
                d.gather_column(content, ctx, seed, x, z);
                // A curtain rooted over our roof only reaches us THROUGH our
                // top row, so an open top row is a free and exact prefilter
                // on the whole margin scan.
                if is_open(ctx, [x, origin[1] + 15, z]) {
                    d.gather_margin(seed, [x, origin[1] + 16, z]);
                }
            }
        }
        d
    }

    /// The floor and ceiling candidates of one column of this section.
    fn gather_column(&mut self, content: &Content, ctx: &GenCtx, seed: u32, x: i32, z: i32) {
        let origin = ctx.origin_world();
        // A colony is a thing on the ground, so its density is the column's.
        let patch_density = patch_at(seed, x, z).0;
        for ly in 0..16 {
            let p = [x, origin[1] + ly, z];
            if !is_open(ctx, p) {
                continue;
            }
            let below = neighbour_rock(content, ctx, p, -1);
            let above = neighbour_rock(content, ctx, p, 1);
            // The rolls come FIRST so an ineligible cell never costs a query
            // slot, and a cell whose classification is still open (rows 0 and
            // 15) is kept for BOTH passes rather than guessed at; the probe
            // decides which one it belongs to.
            let mut ground = GenRng::positional(seed, SALT_GROUND, p[0], p[1], p[2]);
            if ground.next_i32(0, 999) < patch_density && below != Some(false) {
                self.floors.push(Dress { p, below, above });
            }
            let mut roof = GenRng::positional(seed, SALT_CEILING, p[0], p[1], p[2]);
            if roof.next_i32(0, 999) < VINE_PER_MILLE && below != Some(true) && above != Some(false)
            {
                self.ceilings.push(Dress { p, below, above });
            }
        }
    }

    /// The vine roots in the margin rows over one column whose top row is
    /// open; `base` is the first row over the roof.
    fn gather_margin(&mut self, seed: u32, base: [i32; 3]) {
        let [x, y0, z] = base;
        let mut col: Option<usize> = None;
        for k in 0..CEILING_MARGIN {
            let p = [x, y0 + k, z];
            let mut rng = GenRng::positional(seed, SALT_CEILING, p[0], p[1], p[2]);
            if rng.next_i32(0, 999) >= VINE_PER_MILLE {
                continue;
            }
            // The run's LENGTH is the next draw on this very stream, and over
            // half of the margin band's roots stop before reaching our roof.
            // Rolling it here costs one hash and saves the root's query slot
            // and its whole probe block.
            if shroom::vine_len(&mut rng, VINE_MAX_LEN) < k + 2 {
                continue;
            }
            let margin_cols = &mut self.margin_cols;
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
            self.margins.push(Margin { p, col: c });
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.floors.is_empty() && self.ceilings.is_empty() && self.margins.is_empty()
    }

    /// Pay the dressing's two crossings: ONE batched biome query over every
    /// candidate, then ONE batched terrain query over the neighbours the
    /// boundary rows cannot see and the rows over our roof a kept curtain
    /// may hang in from, answered by `biomes` and `terrain` (the host's queries
    /// in play). `None` when a reply came back short.
    pub(super) fn resolve(
        self,
        origin: [i32; 3],
        ours: u8,
        biomes: Query<u8>,
        terrain: Query<TerrainSpace>,
    ) -> Option<Resolved> {
        let mut query: Vec<[i32; 3]> =
            Vec::with_capacity(self.floors.len() + self.ceilings.len() + self.margins.len());
        query.extend(self.floors.iter().map(|d| d.p));
        query.extend(self.ceilings.iter().map(|d| d.p));
        // A margin root is asked about at the ROOT, exactly as an in-section
        // root is: both sections deriving one curtain must gate it the same
        // way, or a curtain near a territory edge comes out in halves.
        query.extend(self.margins.iter().map(|m| m.p));
        let mine: Vec<bool> = probe::ask(query, biomes)?
            .into_iter()
            .map(|b| b == ours)
            .collect();
        let (n_f, n_c) = (self.floors.len(), self.ceilings.len());
        let mut cells: Vec<[i32; 3]> = Vec::new();
        // A floor cell only ever needs its SUPPORT; asking about a roof it
        // does not read would buy a crossing for nothing.
        for (d, _) in self.floors.iter().zip(&mine).filter(|&(_, &m)| m) {
            if d.below.is_none() {
                cells.push([d.p[0], d.p[1] - 1, d.p[2]]);
            }
        }
        for (d, _) in self.ceilings.iter().zip(&mine[n_f..]).filter(|&(_, &m)| m) {
            cells.extend(d.unseen());
        }
        // A margin column is only worth probing once one of its roots is in
        // the biome — otherwise every deep cave in the world would pay seven
        // terrain point queries per open roof column.
        let mut col_probed = vec![false; self.margin_cols.len()];
        for (m, _) in self
            .margins
            .iter()
            .zip(&mine[n_f + n_c..])
            .filter(|&(_, &k)| k)
        {
            if !std::mem::replace(&mut col_probed[m.col], true) {
                let MarginCol { xz: [x, z], rows } = self.margin_cols[m.col];
                cells.extend((0..rows).map(|k| [x, origin[1] + 16 + k as i32, z]));
            }
        }
        let mut reads = TerrainReads::with_query(terrain);
        reads.ask(cells).then_some(Resolved {
            dressing: self,
            mine,
            reads,
        })
    }
}

impl Resolved {
    /// Emit the kept floors, ceilings and margin curtains, in that order.
    pub(super) fn emit(&self, content: &Content, out: &mut Emitter, seed: u32) {
        let d = &self.dressing;
        let (n_f, n_c) = (d.floors.len(), d.ceilings.len());
        for (dress, _) in d.floors.iter().zip(&self.mine).filter(|&(_, &m)| m) {
            if dress.on_rock(&self.reads) {
                emit_floor(content, out, seed, dress.p);
            }
        }
        let ctx = out.ctx();
        for (dress, _) in d
            .ceilings
            .iter()
            .zip(&self.mine[n_f..])
            .filter(|&(_, &m)| m)
        {
            // A cell resting on rock is a FLOOR, whatever hangs over it;
            // letting both passes claim one would put two decorations in one
            // cell.
            if dress.on_rock(&self.reads) || !dress.under_roof(&self.reads) {
                continue;
            }
            hang_curtain(content, out, seed, dress.p, |cell| is_open(ctx, cell));
        }
        let origin = ctx.origin_world();
        for (m, _) in d
            .margins
            .iter()
            .zip(&self.mine[n_f + n_c..])
            .filter(|&(_, &k)| k)
        {
            // Probe row `j` is `origin.y + 16 + j`; the root sits at `k`, its
            // support at `k - 1` (our own roof row when `k == 0`, which the
            // gather already proved open) and its roof at `k + 1`.
            let [x, _, z] = m.p;
            let row = |j: i32| [x, origin[1] + 16 + j, z];
            let k = m.p[1] - origin[1] - 16;
            let is_rock = |j: i32| self.reads.rock(row(j));
            if is_rock(k) || !is_rock(k + 1) || (k > 0 && is_rock(k - 1)) {
                continue;
            }
            hang_curtain(content, out, seed, m.p, |cell| {
                let j = cell[1] - origin[1] - 16;
                if j >= 0 {
                    self.reads.free(row(j))
                } else {
                    is_open(ctx, cell)
                }
            });
        }
    }
}

/// One kept floor cell: the colony's flower or sporeshroom, if the density
/// roll keeps it.
fn emit_floor(content: &Content, out: &mut Emitter, seed: u32, p: [i32; 3]) {
    let mut rng = GenRng::positional(seed, SALT_GROUND, p[0], p[1], p[2]);
    let roll = rng.next_i32(0, 999); // the same draw the gather pass made
    let (density, flower, salt) = patch_at(seed, p[0], p[2]);
    if roll >= density {
        return;
    }
    // The colony's kind, with a minority of the other so an edge is not a
    // clean species boundary. Drawn from the SAME stream, after the density
    // roll, so the gather pass never has to know about it.
    let minority = rng.next_i32(0, 999) < PATCH_ADMIX_PER_MILLE;
    let species = patch_species(content, seed, salt, p);
    let block = if flower != minority {
        species.flower
    } else {
        species.sporeshroom
    };
    out.push_if_clear(p, block);
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
    content: &Content,
    out: &mut Emitter,
    seed: u32,
    root: [i32; 3],
    open: impl Fn([i32; 3]) -> bool,
) {
    let mut rng = GenRng::positional(seed, SALT_CEILING, root[0], root[1], root[2]);
    let _ = rng.next_i32(0, 999); // the gather roll
    let mut blocked = false;
    shroom::vine_run(&mut rng, VINE_MAX_LEN, |dy| {
        let cell = [root[0], root[1] + dy, root[2]];
        if blocked || out.taken(cell) || !open(cell) {
            blocked = true;
            return;
        }
        out.push_if_clear(cell, vine_at(content, seed, cell));
    });
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
pub(super) fn vine_at(content: &Content, seed: u32, cell: [i32; 3]) -> BlockId {
    let mut rng = GenRng::positional(seed, SALT_VINE_BLOOM, cell[0], cell[1], cell[2]);
    if rng.next_i32(0, 999) < VINE_BLOOM_PER_MILLE {
        pick_species(content, seed, cell[0], cell[1], cell[2]).glow_vine
    } else {
        content.vine
    }
}

/// What grows at a column, and how densely: `(per-mille, kind_is_flower,
/// species_salt)`. The species salt is the COLONY's, not the cell's, so one
/// patch is one colour — the thing that makes it read as a single organism's
/// spread rather than confetti.
pub(super) fn patch_at(seed: u32, wx: i32, wz: i32) -> (i32, bool, i32) {
    let (density, owner) = PATCHES.densest(seed, wx, wz, |rng| {
        let is_flower = rng.next_i32(0, 1) == 1;
        (is_flower, rng.next_i32(0, 1 << 20))
    });
    let (flower, salt) = owner.map_or((false, 0), |colony| colony.traits);
    (density, flower, salt)
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
