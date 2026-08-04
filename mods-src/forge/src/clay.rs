//! Clay deposits: in riverbeds, on the banks beside them, and in patches
//! across savanna.
//!
//! SEAM CONTRACT. Every deposit is decided PER COLUMN, from the column's own
//! coordinates, and writes only into that column. A column is owned by exactly
//! one section, so no two dispatches can disagree about it and nothing needs a
//! cross-section acceptance decision — the shape of a patch comes from a
//! smooth positional field, not from an anchor whose neighbours have to be
//! talked into agreeing with it.
//!
//! WHY THERE IS A HOST CALL IN HERE AT ALL. "Near a river" is not something a
//! column knows about itself. Its own biome is in the dispatch's column map,
//! but its NEIGHBOURS' biomes are not — the map covers this section's 16x16
//! and nothing else — so the bank test needs `surface_biome_at`, the
//! positional query. It is batched to exactly ONE call per dispatch, over the
//! probe offsets of the columns that already passed the free positional roll:
//! a per-column query would be the classic way to trip the mod watchdog.

use mod_sdk::*;

/// Frozen positional salt. Changing it reshuffles clay in every existing world.
const SALT_FIELD: u64 = 0xF012_C1A7_0000_0001;
const SALT_DEPTH: u64 = 0xF012_C1A7_0000_0002;

/// The deposit field's lattice period, in blocks. Patch SIZE and patch SPACING
/// are the same knob for any smoothed-lattice field, so this is chosen for the
/// size a clay bank should read at — a few blocks across — and the thresholds
/// below do the rarity.
const PERIOD: i32 = 24;

/// Field thresholds. A smoothed field of uniform corners clusters hard around
/// 0.5, so these are far less permissive than they look; they are measured
/// numbers, not arithmetic — retune them against a census, never by
/// reasoning about the threshold.
const RIVER_MIN: f32 = 0.70;
const BANK_MIN: f32 = 0.70;
const SAVANNA_MIN: f32 = 0.84;

/// How far a column may be from a river and still count as its bank.
const BANK_REACH: i32 = 4;
const PROBES: [(i32, i32); 4] = [
    (BANK_REACH, 0),
    (-BANK_REACH, 0),
    (0, BANK_REACH),
    (0, -BANK_REACH),
];

/// A deposit is this many blocks thick, top-anchored at the column surface.
const DEPTH_MIN: i32 = 1;
const DEPTH_MAX: i32 = 3;

/// A riverbed or bank sits at the waterline; a column far above it is not one,
/// whatever its neighbours are.
const BANK_MAX_RISE: i32 = 6;

/// Ground a deposit may replace. Anything else — ore, wood, water, another
/// pack's content — is left exactly where it is.
const REPLACEABLE: [&str; 8] = [
    "petramond:grass",
    "petramond:dirt",
    "petramond:coarse_dirt",
    "petramond:podzol",
    "petramond:sand",
    "petramond:red_sand",
    "petramond:gravel",
    "petramond:stone",
];

#[derive(Default)]
pub struct Deposits {
    clay: Option<BlockId>,
    replaceable: Vec<BlockId>,
}

impl Deposits {
    /// Registry resolution only: this runs on the DETACHED per-thread worldgen
    /// instances too, where there is no simulation to call into.
    pub fn init(&mut self) {
        self.clay = resolve_block_logged("petramond:clay");
        // Logged like the clay row above: these are ENGINE names, so one that
        // stops resolving is a rename, and the only symptom without a line in
        // the log is clay quietly ceasing to generate in whole biome families.
        self.replaceable = REPLACEABLE
            .iter()
            .filter_map(|n| resolve_block_logged(n))
            .collect();
    }

    pub fn generate(&self, ctx: &GenCtx) -> Vec<GenWrite> {
        let Some(clay) = self.clay else {
            return Vec::new();
        };
        let origin = ctx.origin_world();
        let (y0, y1) = (origin[1], origin[1] + 16);
        let seed = ctx.seed();

        // Pass 1: the free positional roll, plus the two gates a column can
        // answer on its own.
        let field = FieldTile::over(seed, origin[0], origin[2]);
        let mut accepted: Vec<Column> = Vec::new();
        let mut probing: Vec<Column> = Vec::new();
        ctx.for_each_origin(0, |wx, wz| {
            let Some(surf) = ctx.surface_y(wx, wz) else {
                return;
            };
            // The deposit hangs DOWN from the surface, occupying at most
            // `surf - DEPTH_MAX + 1 ..= surf`. A column whose widest possible
            // band misses this section writes nothing however it rolls — and
            // must be rejected HERE, before it can spend four slots of the
            // batched biome probe on a decision with no cells to apply it to.
            if surf < y0 || surf - DEPTH_MAX + 1 >= y1 {
                return;
            }
            let Some(biome) = ctx.biome(wx, wz) else {
                return;
            };
            let roll = field.at(wx, wz);
            let column = Column { wx, wz, surf };
            match biome {
                biome::RIVER if roll >= RIVER_MIN => accepted.push(column),
                biome::SAVANNA if roll >= SAVANNA_MIN => accepted.push(column),
                b if roll >= BANK_MIN && bankable(b) && surf <= ctx.sea_level() + BANK_MAX_RISE => {
                    probing.push(column)
                }
                _ => {}
            }
        });

        // Pass 2: ONE batched biome query, over the probe ring of the columns
        // that got this far and nothing else.
        if !probing.is_empty() {
            let mut columns = Vec::with_capacity(probing.len() * PROBES.len());
            for c in &probing {
                for (dx, dz) in PROBES {
                    columns.push([c.wx + dx, c.wz + dz]);
                }
            }
            let biomes = surface_biome_at(columns);
            for (i, column) in probing.into_iter().enumerate() {
                let ring = &biomes[i * PROBES.len()..(i + 1) * PROBES.len()];
                if ring.contains(&biome::RIVER) {
                    accepted.push(column);
                }
            }
        }

        // Pass 3: cut the deposits. `ctx.block` is a per-cell CLIP — it answers
        // only for cells this dispatch may emit, which is exactly the question
        // being asked here.
        let mut writes = Vec::new();
        for column in accepted {
            let mut rng = GenRng::positional(seed, SALT_DEPTH, column.wx, 0, column.wz);
            let depth = rng.next_i32(DEPTH_MIN, DEPTH_MAX);
            for y in (column.surf - depth + 1)..=column.surf {
                if y < y0 || y >= y1 {
                    continue;
                }
                let pos = [column.wx, y, column.wz];
                if ctx
                    .block(pos)
                    .is_some_and(|b| self.replaceable.contains(&b))
                {
                    writes.push((pos, clay));
                }
            }
        }
        writes
    }
}

#[derive(Clone, Copy)]
struct Column {
    wx: i32,
    wz: i32,
    surf: i32,
}

/// Biomes whose ground can host a clay bank. Ocean floors and bare peaks
/// cannot: one is not a bank and the other is not soil.
fn bankable(b: u8) -> bool {
    !matches!(
        b,
        biome::OCEAN
            | biome::DEEP_OCEAN
            | biome::RIVER
            | biome::SNOWY_PEAKS
            | biome::STONY_PEAKS
            | biome::MOUNTAINS
    )
}

/// The deposit field over ONE section's columns: a smoothed value-noise field
/// on a `PERIOD` lattice, in `0..1`.
///
/// The lattice corners covering a 16-wide section are gathered ONCE (at most
/// 2x2 of them for `PERIOD >= 16`), so the 256 columns cost four hashes and an
/// interpolation each — no per-column hashing, and nothing keyed by a map in a
/// loop that runs for every section in the world.
struct FieldTile {
    /// Lattice cell of the section's origin column.
    cx: i32,
    cz: i32,
    /// Corner values, row-major over `(cx..=cx+span_x, cz..=cz+span_z)`.
    corners: Vec<f32>,
    stride: usize,
}

impl FieldTile {
    fn over(seed: u32, x0: i32, z0: i32) -> FieldTile {
        let cx = x0.div_euclid(PERIOD);
        let cz = z0.div_euclid(PERIOD);
        // +1 for the far edge of the last cell the section reaches into.
        let span_x = ((x0 + 15).div_euclid(PERIOD) - cx + 1) as usize + 1;
        let span_z = ((z0 + 15).div_euclid(PERIOD) - cz + 1) as usize + 1;
        let mut corners = Vec::with_capacity(span_x * span_z);
        for iz in 0..span_z as i32 {
            for ix in 0..span_x as i32 {
                corners.push(GenRng::positional(seed, SALT_FIELD, cx + ix, 0, cz + iz).next_f32());
            }
        }
        FieldTile {
            cx,
            cz,
            corners,
            stride: span_x,
        }
    }

    fn at(&self, wx: i32, wz: i32) -> f32 {
        let ix = (wx.div_euclid(PERIOD) - self.cx) as usize;
        let iz = (wz.div_euclid(PERIOD) - self.cz) as usize;
        let sx = smoothstep(wx.rem_euclid(PERIOD) as f32 / PERIOD as f32);
        let sz = smoothstep(wz.rem_euclid(PERIOD) as f32 / PERIOD as f32);
        let corner = |dx: usize, dz: usize| self.corners[(iz + dz) * self.stride + ix + dx];
        let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
        let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
        top + (bottom - top) * sz
    }
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
