use mod_sdk::*;

use super::claims::Emitter;
use super::{
    is_open, neighbour_rock, pick_species, CEILING_MARGIN, SALT_CEILING, SALT_GROUND, SALT_PATCH,
    SALT_VINE_BLOOM, VINE_MAX_LEN,
};
use crate::content::{Content, Species};
use crate::probe::{self, Query, TerrainReads};
use crate::shroom;

const VINE_PER_MILLE: i32 = 55;
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
const PATCH_ONE_IN: i32 = 2;
const PATCH_R: (i32, i32) = (3, 7);
const PATCH_CORE_PER_MILLE: i32 = 340;
const PATCH_RIM_PER_MILLE: i32 = 40;
pub(super) const STRAY_PER_MILLE: i32 = 3;
const PATCHES: ColonyField = ColonyField {
    salt: SALT_PATCH,
    lattice: PATCH_LATTICE,
    one_in: PATCH_ONE_IN,
    radius: PATCH_R,
    core: PATCH_CORE_PER_MILLE,
    rim: PATCH_RIM_PER_MILLE,
    stray: STRAY_PER_MILLE,
};
const PATCH_ADMIX_PER_MILLE: i32 = 120;

pub(super) struct Dress {
    pub(super) p: [i32; 3],
    pub(super) below: Option<bool>,
    pub(super) above: Option<bool>,
}

impl Dress {
    pub(super) fn unseen(&self) -> Option<[i32; 3]> {
        match (self.below, self.above) {
            (None, _) => Some([self.p[0], self.p[1] - 1, self.p[2]]),
            (_, None) => Some([self.p[0], self.p[1] + 1, self.p[2]]),
            _ => None,
        }
    }

    fn on_rock(&self, reads: &TerrainReads) -> bool {
        self.below
            .unwrap_or_else(|| reads.rock([self.p[0], self.p[1] - 1, self.p[2]]))
    }

    fn under_roof(&self, reads: &TerrainReads) -> bool {
        self.above
            .unwrap_or_else(|| reads.rock([self.p[0], self.p[1] + 1, self.p[2]]))
    }
}

pub(super) struct Margin {
    pub(super) p: [i32; 3],
    pub(super) col: usize,
}

pub(super) struct MarginCol {
    pub(super) xz: [i32; 2],
    pub(super) rows: usize,
}

#[derive(Default)]
pub(super) struct Dressing {
    pub(super) floors: Vec<Dress>,
    pub(super) ceilings: Vec<Dress>,
    pub(super) margins: Vec<Margin>,
    pub(super) margin_cols: Vec<MarginCol>,
}

pub(super) struct Resolved {
    dressing: Dressing,
    mine: Vec<bool>,
    reads: TerrainReads,
}

impl Dressing {
    pub(super) fn gather(content: &Content, ctx: &GenCtx, seed: u32) -> Dressing {
        let origin = ctx.origin_world();
        let mut d = Dressing::default();
        for lz in 0..16 {
            for lx in 0..16 {
                let (x, z) = (origin[0] + lx, origin[2] + lz);
                d.gather_column(content, ctx, seed, x, z);
                if is_open(ctx, [x, origin[1] + 15, z]) {
                    d.gather_margin(seed, [x, origin[1] + 16, z]);
                }
            }
        }
        d
    }

    fn gather_column(&mut self, content: &Content, ctx: &GenCtx, seed: u32, x: i32, z: i32) {
        let origin = ctx.origin_world();
        let patch_density = patch_at(seed, x, z).0;
        for ly in 0..16 {
            let p = [x, origin[1] + ly, z];
            if !is_open(ctx, p) {
                continue;
            }
            let below = neighbour_rock(content, ctx, p, -1);
            let above = neighbour_rock(content, ctx, p, 1);
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

    fn gather_margin(&mut self, seed: u32, base: [i32; 3]) {
        let [x, y0, z] = base;
        let mut col: Option<usize> = None;
        for k in 0..CEILING_MARGIN {
            let p = [x, y0 + k, z];
            let mut rng = GenRng::positional(seed, SALT_CEILING, p[0], p[1], p[2]);
            if rng.next_i32(0, 999) >= VINE_PER_MILLE {
                continue;
            }
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
            margin_cols[c].rows = margin_cols[c].rows.max(k as usize + 2);
            self.margins.push(Margin { p, col: c });
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.floors.is_empty() && self.ceilings.is_empty() && self.margins.is_empty()
    }

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
        query.extend(self.margins.iter().map(|m| m.p));
        let mine: Vec<bool> = probe::ask(query, biomes)?
            .into_iter()
            .map(|b| b == ours)
            .collect();
        let (n_f, n_c) = (self.floors.len(), self.ceilings.len());
        let mut cells: Vec<[i32; 3]> = Vec::new();
        for (d, _) in self.floors.iter().zip(&mine).filter(|&(_, &m)| m) {
            if d.below.is_none() {
                cells.push([d.p[0], d.p[1] - 1, d.p[2]]);
            }
        }
        for (d, _) in self.ceilings.iter().zip(&mine[n_f..]).filter(|&(_, &m)| m) {
            cells.extend(d.unseen());
        }
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

fn emit_floor(content: &Content, out: &mut Emitter, seed: u32, p: [i32; 3]) {
    let mut rng = GenRng::positional(seed, SALT_GROUND, p[0], p[1], p[2]);
    let roll = rng.next_i32(0, 999);
    let (density, flower, salt) = patch_at(seed, p[0], p[2]);
    if roll >= density {
        return;
    }
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
    let _ = rng.next_i32(0, 999);
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

pub(super) fn vine_at(content: &Content, seed: u32, cell: [i32; 3]) -> BlockId {
    let mut rng = GenRng::positional(seed, SALT_VINE_BLOOM, cell[0], cell[1], cell[2]);
    if rng.next_i32(0, 999) < VINE_BLOOM_PER_MILLE {
        pick_species(content, seed, cell[0], cell[1], cell[2]).glow_vine
    } else {
        content.vine
    }
}

pub(super) fn patch_at(seed: u32, wx: i32, wz: i32) -> (i32, bool, i32) {
    let (density, owner) = PATCHES.densest(seed, wx, wz, |rng| {
        let is_flower = rng.next_i32(0, 1) == 1;
        (is_flower, rng.next_i32(0, 1 << 20))
    });
    let (flower, salt) = owner.map_or((false, 0), |colony| colony.traits);
    (density, flower, salt)
}

fn patch_species(content: &Content, seed: u32, salt: i32, p: [i32; 3]) -> &Species {
    if salt == 0 {
        return pick_species(content, seed, p[0], p[1], p[2]);
    }
    &content.species[(salt as usize) % content.species.len()]
}
