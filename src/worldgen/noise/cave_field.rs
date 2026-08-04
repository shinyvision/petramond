//! Cave field and active cave carving helpers.
//!
//! Cave decisions are plain typed functions of world position plus the column's
//! original density surface, so caves are identical from every chunk/section that
//! touches them: seamless tunnels and entrances with no inter-chunk state.
//!
//! Three interior carvers (spaghetti, noodle, cheese — see
//! [`super::settings`]) plus surface entrances, and a very-low-frequency
//! UNDERGROUND-BIOME field. Every carver also computes a wall "shell": a solid
//! voxel whose carve metric lands within [`CAVE_LINING_SHELL`] of the carve
//! threshold hugs a cave wall, and the biome owning that voxel paints the shell
//! its lining block. Because the shell is a pure function of the same fields as
//! the carve, wall lining needs no neighbour queries and stays seam-free.
//!
//! WHICH biome owns a band of that field, what it lines its walls with, and how
//! roomy its caves are is DATA
//! ([`crate::worldgen::data::underground`]) — this module knows only "a biome
//! id, its lining block id, and its caliber". Caliber (tunnel/cheese/shell
//! multipliers) applies to the INTERIOR arm only: the entrance arm feeds the
//! hottest point query in worldgen, and making it biome-dependent would drag
//! the biome field into every surface column in the world.
//!
//! The interior fields are sampled on a world-anchored [`LATTICE_STEP`]-block
//! lattice and trilinearly interpolated per voxel — the highest field frequency
//! (noodle Y, 0.039 ≈ 26-block wavelength) is far below the lattice's Nyquist
//! limit, and per-voxel OpenSimplex sampling was ~a quarter of all worldgen CPU.
//! Anchoring lattice points to absolute multiples of `LATTICE_STEP` makes every
//! path — per-section carve, whole-chunk carve, per-point surface walks — read
//! identical values, so caves stay seamless and column heightmaps stay consistent
//! with carved blocks. The entrance GATE fields stay exactly-sampled: they are
//! evaluated once per column (hot in `ColumnGen`) where a lazy point lattice
//! would cost more than the two samples they replace.

use super::settings::*;

use super::simplex::Simplex3;
use crate::block::Block;
use crate::chunk::{idx, section_idx, Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_SIZE};
use crate::section::Section;
use crate::worldgen::data::underground::{self, LiningFaces, UndergroundBiomes};

use super::chamber;

mod batch;
mod territory;

const LATTICE_STEP: i32 = CAVE_LATTICE_STEP;
const LATTICE_STEP_F: f64 = LATTICE_STEP as f64;

/// What the carvers decide for one solid voxel: carve it open, leave it but line
/// it (it hugs a cave wall), or leave it untouched.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum CaveCut {
    Solid,
    Shell,
    Open,
}

/// Owns the cave noise samplers and decides whether a solid voxel is carved to
/// air (or lined by its underground biome). Immutable after construction;
/// `Send + Sync`.
///
/// Each sampler is salt-seeded (`Simplex3::new(seed.wrapping_add(SALT_CAVE_*))`)
/// so construction order is irrelevant and output is a pure function of seed.
pub struct CaveField {
    seed: u32,
    cave_a: Simplex3, // spaghetti tunnel field A (shared by main + branch)
    cave_b: Simplex3, // spaghetti tunnel field B (main system)
    cave_c: Simplex3, // cheese cavern field
    /// Spaghetti BRANCH field: `max(|a|,|branch|)` forms a second tunnel family
    /// on the same `a ≈ 0` sheet as the main system, so the two families cross
    /// at isolated points — natural forks and junctions.
    branch: Simplex3,
    noodle_a: Simplex3,
    noodle_b: Simplex3,
    roughness: Simplex3,
    biome: Simplex3,
    entrance_a: Simplex3,
    entrance_b: Simplex3,
    /// The loaded underground-biome partition (lining + caliber per biome).
    underground: &'static UndergroundBiomes,
    /// Hoisted from `underground`: whether any banded row modulates caliber.
    caliber_varies: bool,
    /// Hoisted from `underground`: the inclusive Y span outside which no
    /// declared chamber can contribute anything, or `None` when none exist.
    chamber_y_span: Option<(i32, i32)>,
    /// Hoisted from `underground`: whether any row lines its cave surfaces
    /// per ORIENTATION. Arms the extra lattice row the floor probe needs, the
    /// skip mask's dilation, and the column bookkeeping — all of which a table
    /// without it must not pay for.
    lining_faces: bool,
}

/// Which field groups a lattice must carry. A skipped group is neither sampled
/// nor ALLOCATED, so a caller that reads less pays less — the sparse queries
/// run per surface column and per ABI position, where four unused samplers and
/// five unused vectors per call are the whole cost.
#[derive(Copy, Clone)]
struct Fields {
    /// Spaghetti A/B + roughness: every CARVE decision reads them, both arms.
    carve: bool,
    /// The remaining interior carvers: branch, noodle, cheese.
    interior: bool,
    /// The underground-biome partition field.
    biome: bool,
}

impl Fields {
    const ALL: Fields = Fields {
        carve: true,
        interior: true,
        biome: true,
    };
}

/// The interior cave fields of one axis-aligned box, sampled at every world
/// lattice point the box touches. Built per carve batch (section / chunk) or,
/// degenerately, per point for the rare surface walks.
struct CaveLattice {
    lx0: i32,
    ly0: i32,
    lz0: i32,
    nx: usize,
    ny: usize,
    nz: usize,
    a: Vec<f64>,
    b: Vec<f64>,
    branch: Vec<f64>,
    na: Vec<f64>,
    nb: Vec<f64>,
    rough: Vec<f64>,
    cheese: Vec<f64>,
    biome: Vec<f64>,
    /// Data-declared additive room term. Unlike every other lane this one may
    /// legitimately be ABSENT rather than merely unread: it is skipped only for
    /// boxes where no declared chamber can reach, where its value is provably
    /// `0.0`. So [`CaveLattice::chamber`] answers zero instead of asserting —
    /// the producer's gate and the consumer's read cannot disagree about a
    /// value they both know is zero.
    chamber: Vec<f64>,
    /// The chamber's second lane: how much a room widens the RADIUS carvers
    /// here. Absent both when no room reaches the box and when no room that
    /// does declares a gain, which is the same "provably `0.0`" argument.
    chamber_tunnel: Vec<f64>,
    fields: Fields,
}

/// The lattice lanes a carve decision can read, as cache slots for [`Col`].
mod lane {
    pub(super) const A: usize = 0;
    pub(super) const B: usize = 1;
    pub(super) const ROUGH: usize = 2;
    pub(super) const BRANCH: usize = 3;
    pub(super) const NA: usize = 4;
    pub(super) const NB: usize = 5;
    pub(super) const CHEESE: usize = 6;
    pub(super) const BIOME: usize = 7;
    pub(super) const CHAMBER: usize = 8;
    pub(super) const TUNNEL: usize = 9;
    pub(super) const COUNT: usize = 10;
}

/// A fixed-`(x, z)` cursor into a lattice.
///
/// The carve decision reads up to eight lanes per voxel, and a plain
/// [`CaveLattice::tri`] redoes the cell index, the three fractions and the two
/// X lerps and one Z lerp for every one of them. But X and Z are CONSTANT down
/// a column, and the four voxels of a lattice cell share its two Y planes, so
/// the whole xz part of the interpolation is computed once per lane per cell
/// and each voxel is left with a single lerp. The lerp sequence is exactly
/// `tri`'s, so the value is bit-identical — this is a schedule, not a formula.
struct Col<'a> {
    lat: &'a CaveLattice,
    /// Corner offsets of the column's four xz neighbours within one Y plane.
    i00: usize,
    i01: usize,
    plane: usize,
    tx: f64,
    tz: f64,
    /// Lattice cell the cached planes belong to (`i32::MIN` = nothing cached).
    cell_y: i32,
    cached: u16,
    lo: [f64; lane::COUNT],
    hi: [f64; lane::COUNT],
}

impl<'a> Col<'a> {
    #[inline]
    fn new(lat: &'a CaveLattice, x: i32, z: i32) -> Self {
        let cx = (x.div_euclid(LATTICE_STEP) - lat.lx0) as usize;
        let cz = (z.div_euclid(LATTICE_STEP) - lat.lz0) as usize;
        debug_assert!(cx + 1 < lat.nx && cz + 1 < lat.nz);
        Self {
            lat,
            i00: cz * lat.nx + cx,
            i01: (cz + 1) * lat.nx + cx,
            plane: lat.nz * lat.nx,
            tx: x.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F,
            tz: z.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F,
            cell_y: i32::MIN,
            cached: 0,
            lo: [0.0; lane::COUNT],
            hi: [0.0; lane::COUNT],
        }
    }

    /// The lane's xz-interpolated value on the Y plane starting at `base`.
    #[inline]
    fn bilinear(&self, field: &[f64], base: usize) -> f64 {
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let x0 = lerp(field[base + self.i00], field[base + self.i00 + 1], self.tx);
        let x1 = lerp(field[base + self.i01], field[base + self.i01 + 1], self.tx);
        lerp(x0, x1, self.tz)
    }

    #[inline]
    fn get(&mut self, k: usize, y: i32) -> f64 {
        let cy = y.div_euclid(LATTICE_STEP) - self.lat.ly0;
        if cy != self.cell_y {
            self.cell_y = cy;
            self.cached = 0;
        }
        if self.cached & (1 << k) == 0 {
            let lat = self.lat;
            let field: &[f64] = match k {
                lane::A => &lat.a,
                lane::B => &lat.b,
                lane::ROUGH => &lat.rough,
                lane::BRANCH => &lat.branch,
                lane::NA => &lat.na,
                lane::NB => &lat.nb,
                lane::CHEESE => &lat.cheese,
                lane::BIOME => &lat.biome,
                lane::CHAMBER => &lat.chamber,
                _ => &lat.chamber_tunnel,
            };
            debug_assert!(
                !field.is_empty(),
                "lattice built without a field group this decision reads"
            );
            debug_assert!((cy as usize) + 1 < lat.ny);
            let base = cy as usize * self.plane;
            self.lo[k] = self.bilinear(field, base);
            self.hi[k] = self.bilinear(field, base + self.plane);
            self.cached |= 1 << k;
        }
        let ty = y.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F;
        self.lo[k] + (self.hi[k] - self.lo[k]) * ty
    }

    /// The chamber lane answers zero when absent — see [`CaveLattice::chamber`].
    #[inline]
    fn chamber(&mut self, y: i32) -> f64 {
        if self.lat.chamber.is_empty() {
            return 0.0;
        }
        self.get(lane::CHAMBER, y)
    }
}

impl CaveLattice {
    /// Trilinear interpolation of `field` at world voxel `(x,y,z)` (must lie inside
    /// the box the lattice was built for).
    #[inline]
    fn tri(&self, field: &[f64], x: i32, y: i32, z: i32) -> f64 {
        // A group this lattice was built without stays empty; naming that turns
        // an opaque bounds panic in the deepest query path into the actual bug.
        debug_assert!(
            !field.is_empty(),
            "lattice built without a field group this decision reads"
        );
        let cx = (x.div_euclid(LATTICE_STEP) - self.lx0) as usize;
        let cy = (y.div_euclid(LATTICE_STEP) - self.ly0) as usize;
        let cz = (z.div_euclid(LATTICE_STEP) - self.lz0) as usize;
        let tx = x.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F;
        let ty = y.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F;
        let tz = z.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F;
        debug_assert!(cx + 1 < self.nx && cy + 1 < self.ny && cz + 1 < self.nz);

        let i =
            |dx: usize, dy: usize, dz: usize| ((cy + dy) * self.nz + cz + dz) * self.nx + cx + dx;
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let x00 = lerp(field[i(0, 0, 0)], field[i(1, 0, 0)], tx);
        let x01 = lerp(field[i(0, 0, 1)], field[i(1, 0, 1)], tx);
        let x10 = lerp(field[i(0, 1, 0)], field[i(1, 1, 0)], tx);
        let x11 = lerp(field[i(0, 1, 1)], field[i(1, 1, 1)], tx);
        let z0 = lerp(x00, x01, tz);
        let z1 = lerp(x10, x11, tz);
        lerp(z0, z1, ty)
    }

    /// Conservative per-cell "any voxel here might open or shell" mask, one
    /// flag per LATTICE_STEP³ cell. Trilinear values are convex combinations
    /// of the 8 cell corners, so per-field corner min/max bound every
    /// interpolated voxel in the cell; a cell whose bounds clear every carver
    /// family's most permissive radius (shell margins included) is provably
    /// all-`CaveCut::Solid` and its voxels can be skipped wholesale —
    /// byte-identical by construction. Indexed `(cy * (nz-1) + cz) * (nx-1) + cx`.
    ///
    /// The caliber bound a cell widens by is the widest ANY row reaching THAT
    /// cell can produce (see [`UndergroundBiomes::bounds_for`]) — a cell can
    /// straddle a band boundary, so a bound narrower than what the carver would
    /// actually open silently deletes caves with no other symptom. Taking it
    /// per cell rather than over the whole table is what keeps a row banded to
    /// a depth or a field value this box never reaches from costing it
    /// anything: the cell's Y range is exact, and its field range is bounded by
    /// the same 8 biome corners every other bound here comes from.
    ///
    /// `floor_lining` DILATES the mask one cell upward: an orientation lining
    /// paints the rock UNDER carved air, which can sit in the cell below the
    /// one the air is in. Without the dilation that floor cell is skipped as
    /// provably solid — true, and irrelevant, because it is still being
    /// painted. The symptom would be moss that appears or not depending on
    /// where the 4-block lattice boundary falls.
    fn may_cut_mask(&self, biomes: &UndergroundBiomes, floor_lining: bool) -> Vec<bool> {
        let (mx, my, mz) = (self.nx - 1, self.ny - 1, self.nz - 1);
        let mut mask = vec![true; mx * my * mz];
        let corner = |f: &[f64], cx: usize, cy: usize, cz: usize, d: usize| {
            f[((cy + (d >> 2 & 1)) * self.nz + cz + (d >> 1 & 1)) * self.nx + cx + (d & 1)]
        };
        let bounds = |f: &[f64], cx: usize, cy: usize, cz: usize| {
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for d in 0..8 {
                let v = corner(f, cx, cy, cz, d);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            (lo, hi)
        };
        let abs_lb = |(lo, hi): (f64, f64)| {
            if lo > 0.0 {
                lo
            } else if hi < 0.0 {
                -hi
            } else {
                0.0
            }
        };

        // Whole-box bound first. Cell windows nest inside the box window and
        // `bounds_for` is monotone in both, so a box no banded row reaches
        // needs no per-cell narrowing at all — the common case, and it keeps
        // this loop exactly as cheap as it was before rows could be banded.
        let box_ub = if self.fields.biome {
            let mut f = (f64::INFINITY, f64::NEG_INFINITY);
            for &v in &self.biome {
                f = (f.0.min(v), f.1.max(v));
            }
            let y = (
                self.ly0 * LATTICE_STEP,
                (self.ly0 + my as i32 - 1) * LATTICE_STEP + LATTICE_STEP - 1,
            );
            biomes.bounds_for(y, f)
        } else {
            // No biome field sampled: nothing to narrow with, so the whole
            // table's maximum is the only sound bound.
            biomes.bounds
        };
        let per_cell = self.fields.biome && box_ub != biomes.base_bounds;

        for cy in 0..my {
            let wy_lo = (self.ly0 + cy as i32) * LATTICE_STEP;
            let wy_hi = wy_lo + LATTICE_STEP - 1;
            // cheese_threshold is monotone in y, so the cell max is at an end.
            let cheese_t = cheese_threshold(wy_lo).max(cheese_threshold(wy_hi));
            for cz in 0..mz {
                for cx in 0..mx {
                    let ub = if per_cell {
                        biomes.bounds_for((wy_lo, wy_hi), bounds(&self.biome, cx, cy, cz))
                    } else {
                        box_ub
                    };
                    let (rough_lo, rough_hi) = bounds(&self.rough, cx, cy, cz);
                    let rough_ub = |scale: f64| (rough_lo * scale).max(rough_hi * scale);

                    let a_lb = abs_lb(bounds(&self.a, cx, cy, cz));
                    let ab_lb = a_lb.max(abs_lb(bounds(&self.b, cx, cy, cz)));

                    // A chamber's terms are bounded from the sampled lanes'
                    // own corner maxima — exact, and free — rather than from a
                    // declared cap: a declared cap would be a global constant
                    // added inside the row's whole territory, which is
                    // precisely how `bounds_for` stopped the mask skipping
                    // anything. An absent lane means no chamber can reach this
                    // box at all, i.e. a true bound of zero.
                    //
                    // Both lanes have to be read BEFORE the tunnel test, not
                    // just before the cheese one: the tunnel gain widens the
                    // radius carvers, and a bound taken after their test would
                    // let the mask skip cells the fattened tunnel opens.
                    let chamber_hi = if self.chamber.is_empty() {
                        0.0
                    } else {
                        bounds(&self.chamber, cx, cy, cz).1
                    };
                    let tunnel_gain_ub = if self.chamber_tunnel.is_empty() {
                        1.0
                    } else {
                        1.0 + bounds(&self.chamber_tunnel, cx, cy, cz).1
                    };

                    // Entrance and tunnel both open on the a/b metric; bound
                    // them with the largest radius either can reach here. The
                    // entrance radius is never caliber-scaled or chamber-
                    // widened, but `ub.shell` is floored at 1.0 so its unscaled
                    // shell stays covered.
                    let shell_ub = CAVE_LINING_SHELL * ub.shell;
                    let tunnel_r_ub = (CAVE_TUNNEL_R + rough_ub(CAVE_TUNNEL_ROUGHNESS)).max(0.018)
                        * ub.tunnel
                        * tunnel_gain_ub;
                    let entrance_r_ub = (CAVE_ENTRANCE_SURFACE_R.max(CAVE_ENTRANCE_DEEP_R)
                        + rough_ub(CAVE_TUNNEL_ROUGHNESS))
                    .max(0.016);
                    if ab_lb < tunnel_r_ub.max(entrance_r_ub) + shell_ub {
                        continue;
                    }
                    let branch_lb = a_lb.max(abs_lb(bounds(&self.branch, cx, cy, cz)));
                    if branch_lb < tunnel_r_ub * CAVE_BRANCH_R_SCALE + shell_ub {
                        continue;
                    }
                    if rough_lo < CAVE_NOODLE_GATE_T {
                        let noodle_lb = abs_lb(bounds(&self.na, cx, cy, cz))
                            .max(abs_lb(bounds(&self.nb, cx, cy, cz)));
                        if noodle_lb < CAVE_NOODLE_R * ub.tunnel * tunnel_gain_ub + shell_ub {
                            continue;
                        }
                    }
                    // A chamber's threshold term rides the SAME threshold as
                    // the cavern carver, so it widens this bound and nothing
                    // else.
                    let (cheese_lo, _) = bounds(&self.cheese, cx, cy, cz);
                    if cheese_lo
                        < cheese_t
                            + ub.cheese
                            + chamber_hi
                            + rough_ub(CAVE_CHEESE_ROUGHNESS)
                            + CAVE_CHEESE_LINING_SHELL * ub.shell
                    {
                        continue;
                    }
                    mask[(cy * mz + cz) * mx + cx] = false;
                }
            }
        }
        if floor_lining {
            let base = mask.clone();
            for cy in 0..my {
                for cz in 0..mz {
                    for cx in 0..mx {
                        let above = cy + 1 == my || base[((cy + 1) * mz + cz) * mx + cx];
                        mask[(cy * mz + cz) * mx + cx] |= above;
                    }
                }
            }
        }
        mask
    }

    /// Test seam: whether this box's chamber lane actually carries a room, so
    /// a sweep can prove it is not passing because the term was zero anyway.
    #[cfg(test)]
    fn chamber_is_live(&self) -> bool {
        self.chamber.iter().any(|v| *v != 0.0)
    }

    #[inline]
    fn biome(&self, x: i32, y: i32, z: i32) -> f64 {
        debug_assert!(
            self.fields.biome,
            "lattice built without the underground-biome field"
        );
        self.tri(&self.biome, x, y, z)
    }
}

impl CaveField {
    pub fn new(seed: u32) -> Self {
        Self::with_table(seed, underground::table())
    }

    /// Test seam: drive the carver from a synthetic underground-biome table
    /// without touching the process-wide catalog.
    pub(crate) fn with_table(seed: u32, underground: &'static UndergroundBiomes) -> Self {
        let s = |salt: u32| seed.wrapping_add(salt);
        Self {
            seed,
            underground,
            caliber_varies: underground.caliber_varies,
            chamber_y_span: underground.chamber_y_span,
            lining_faces: underground.lining_faces_vary,
            cave_a: Simplex3::new(s(SALT_CAVE_A)),
            cave_b: Simplex3::new(s(SALT_CAVE_B)),
            cave_c: Simplex3::new(s(SALT_CAVE_C)),
            branch: Simplex3::new(s(SALT_CAVE_BRANCH)),
            noodle_a: Simplex3::new(s(SALT_CAVE_NOODLE_A)),
            noodle_b: Simplex3::new(s(SALT_CAVE_NOODLE_B)),
            roughness: Simplex3::new(s(SALT_CAVE_ROUGHNESS)),
            biome: Simplex3::new(s(SALT_CAVE_BIOME)),
            entrance_a: Simplex3::new(s(SALT_CAVE_ENTRANCE_A)),
            entrance_b: Simplex3::new(s(SALT_CAVE_ENTRANCE_B)),
        }
    }

    // Raw field samplers — the ONE place each field's frequency/offset math lives,
    // so lattice corners and any exact query can never drift apart.
    fn sample_a(&self, x: f64, y: f64, z: f64) -> f64 {
        self.cave_a
            .get([x * CAVE_FREQ_XZ, y * CAVE_FREQ_Y, z * CAVE_FREQ_XZ])
    }
    fn sample_b(&self, x: f64, y: f64, z: f64) -> f64 {
        self.cave_b.get([
            x * CAVE_FREQ_XZ + 13.7,
            y * CAVE_FREQ_Y + 5.1,
            z * CAVE_FREQ_XZ - 7.3,
        ])
    }
    fn sample_branch(&self, x: f64, y: f64, z: f64) -> f64 {
        self.branch.get([
            x * CAVE_FREQ_XZ - 41.3,
            y * CAVE_FREQ_Y + 27.7,
            z * CAVE_FREQ_XZ + 9.1,
        ])
    }
    fn sample_na(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noodle_a.get([
            x * CAVE_NOODLE_FREQ_XZ,
            y * CAVE_NOODLE_FREQ_Y,
            z * CAVE_NOODLE_FREQ_XZ,
        ])
    }
    fn sample_nb(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noodle_b.get([
            x * CAVE_NOODLE_FREQ_XZ - 23.1,
            y * CAVE_NOODLE_FREQ_Y + 17.9,
            z * CAVE_NOODLE_FREQ_XZ + 31.7,
        ])
    }
    fn sample_rough(&self, x: f64, y: f64, z: f64) -> f64 {
        self.roughness.get([
            x * CAVE_ROUGHNESS_FREQ,
            y * CAVE_ROUGHNESS_FREQ * 0.7,
            z * CAVE_ROUGHNESS_FREQ,
        ])
    }
    fn sample_cheese(&self, x: f64, y: f64, z: f64) -> f64 {
        self.cave_c.get([
            x * CAVE_CHEESE_FREQ,
            y * CAVE_CHEESE_FREQ * 1.4,
            z * CAVE_CHEESE_FREQ,
        ])
    }
    fn sample_biome(&self, x: f64, y: f64, z: f64) -> f64 {
        self.biome.get([
            x * CAVE_BIOME_FREQ,
            y * CAVE_BIOME_FREQ_Y,
            z * CAVE_BIOME_FREQ,
        ])
    }

    /// Sample the interior fields at every world lattice point covering the inclusive
    /// voxel box `(x0..=x1, y0..=y1, z0..=z1)` — every field, for the batch carvers.
    fn build_lattice(&self, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32) -> CaveLattice {
        self.build_lattice_filtered(x0, y0, z0, x1, y1, z1, Fields::ALL)
    }

    /// [`build_lattice`] carrying only the field groups the caller's decision
    /// can actually read — the sparse point queries, which run per surface
    /// column and per ABI position. A skipped group stays EMPTY and unallocated
    /// (reading one is a bug, not a wrong value) and a sampled group is
    /// bit-identical to a full lattice, so point and batch decisions can never
    /// drift apart.
    ///
    /// The CHAMBER lane is the one exception, and deliberately so: it is
    /// skipped for boxes where its value is provably `0.0`, and reading it
    /// answers that zero rather than asserting. A gate on "is this group
    /// sampled" would then have to be duplicated in the consumer, which is
    /// exactly the producer/consumer drift the rest of this scheme avoids.
    #[allow(clippy::too_many_arguments)]
    fn build_lattice_filtered(
        &self,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        fields: Fields,
    ) -> CaveLattice {
        let lx0 = x0.div_euclid(LATTICE_STEP);
        let ly0 = y0.div_euclid(LATTICE_STEP);
        let lz0 = z0.div_euclid(LATTICE_STEP);
        let nx = (x1.div_euclid(LATTICE_STEP) + 1 - lx0) as usize + 1;
        let ny = (y1.div_euclid(LATTICE_STEP) + 1 - ly0) as usize + 1;
        let nz = (z1.div_euclid(LATTICE_STEP) + 1 - lz0) as usize + 1;
        let n = nx * ny * nz;
        let cap = |want: bool| if want { n } else { 0 };
        let mut lat = CaveLattice {
            lx0,
            ly0,
            lz0,
            nx,
            ny,
            nz,
            a: Vec::with_capacity(cap(fields.carve)),
            b: Vec::with_capacity(cap(fields.carve)),
            branch: Vec::with_capacity(cap(fields.interior)),
            na: Vec::with_capacity(cap(fields.interior)),
            nb: Vec::with_capacity(cap(fields.interior)),
            rough: Vec::with_capacity(cap(fields.carve)),
            cheese: Vec::with_capacity(cap(fields.interior)),
            biome: Vec::with_capacity(cap(fields.biome)),
            chamber: Vec::new(),
            chamber_tunnel: Vec::new(),
            fields,
        };

        // Chambers feed the cheese arm, so they matter only where the interior
        // carvers run, and only inside the declared depth span. Outside it the
        // term is provably zero, so skipping the lane cannot change an answer —
        // and a table declaring no chamber (every shipped one) never reaches
        // past this `Option`.
        let rooms = self
            .chamber_y_span
            .filter(|_| fields.interior)
            .and_then(|(lo, hi)| {
                // Corner extents, which is what a chamber must be gathered over: a
                // box's `+1` corner belongs to its neighbour too, so the term there
                // has to be the neighbour's answer as well.
                let clo = [lx0 * LATTICE_STEP, ly0 * LATTICE_STEP, lz0 * LATTICE_STEP];
                let chi = [
                    clo[0] + (nx as i32 - 1) * LATTICE_STEP,
                    clo[1] + (ny as i32 - 1) * LATTICE_STEP,
                    clo[2] + (nz as i32 - 1) * LATTICE_STEP,
                ];
                if chi[1] < lo || clo[1] > hi {
                    return None;
                }
                let rooms = chamber::ChamberField::gather(
                    self.underground,
                    self.seed,
                    clo,
                    chi,
                    |x, y, z| self.sample_biome(x as f64, y as f64, z as f64),
                );
                (!rooms.is_empty()).then(|| {
                    lat.chamber.reserve_exact(n);
                    if rooms.any_tunnel() {
                        lat.chamber_tunnel.reserve_exact(n);
                    }
                    rooms
                })
            });
        let tunnel_lane = rooms.as_ref().is_some_and(|r| r.any_tunnel());

        for ly in 0..ny {
            let wy = (ly0 + ly as i32) * LATTICE_STEP;
            let fy = wy as f64;
            for lz in 0..nz {
                let wz = (lz0 + lz as i32) * LATTICE_STEP;
                let fz = wz as f64;
                for lx in 0..nx {
                    let wx = (lx0 + lx as i32) * LATTICE_STEP;
                    let fx = wx as f64;
                    if fields.carve {
                        lat.a.push(self.sample_a(fx, fy, fz));
                        lat.b.push(self.sample_b(fx, fy, fz));
                        lat.rough.push(self.sample_rough(fx, fy, fz));
                    }
                    let mut knead = 0.0;
                    if fields.interior {
                        knead = self.sample_na(fx, fy, fz);
                        lat.branch.push(self.sample_branch(fx, fy, fz));
                        lat.na.push(knead);
                        lat.nb.push(self.sample_nb(fx, fy, fz));
                        lat.cheese.push(self.sample_cheese(fx, fy, fz));
                    }
                    if fields.biome {
                        lat.biome.push(self.sample_biome(fx, fy, fz));
                    }
                    // A room's rim and floor are kneaded by a cave field the
                    // lattice already carries at this exact corner (a chamber
                    // lane exists only where `interior` sampled it), so the
                    // shape stays a pure function of position and costs no
                    // extra sampler.
                    if let Some(rooms) = &rooms {
                        let (v, t) = rooms.at(wx, wy, wz, knead);
                        lat.chamber.push(v);
                        if tunnel_lane {
                            lat.chamber_tunnel.push(t);
                        }
                    }
                }
            }
        }
        lat
    }

    /// Should the solid voxel at world `(x,y,z)` be carved to air? `surf_y` is the
    /// original density top-solid surface for the voxel's `(x,z)` column.
    ///
    /// Point-query form: gates first (exact, cheap), then a degenerate one-voxel
    /// lattice — the SAME evaluator as the batch carve, so surface walks always agree
    /// with carved blocks. Only for sparse queries; batches use [`carve_section`] /
    /// [`carve_chunk`], which amortize one lattice over the whole box.
    pub fn cave_carved(&self, x: i32, y: i32, z: i32, surf_y: i32) -> bool {
        if y > surf_y {
            return false;
        }
        let gate = self.entrance_gate_ease(x, y, z, surf_y);
        let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
        if gate.is_none() && !interior {
            return false;
        }
        // Sample only the fields this decision can read: the non-interior
        // (entrance-band) query is the hot one — the per-column surface probes —
        // and it needs just the spaghetti + roughness fields. The biome field
        // is read IFF the interior arm runs AND a banded row modulates caliber,
        // which is exactly the condition `cut_from_lattice` branches on, so
        // producer and consumer cannot drift.
        let fields = Fields {
            carve: true,
            interior,
            biome: interior && self.caliber_varies,
        };
        let lat = self.build_lattice_filtered(x, y, z, x, y, z, fields);
        self.cut_from_col(&mut Col::new(&lat, x, z), y, gate, interior) == CaveCut::Open
    }

    /// The one carve decision, given interpolated interior fields. `gate` and
    /// `interior` are precomputed by the caller (the point path uses them to skip
    /// building a lattice at all for the common solid voxel). Returns `Open` when
    /// a carver cuts the voxel, `Shell` when the voxel survives but sits within a
    /// carver's lining shell of a wall, `Solid` otherwise.
    #[inline]
    fn cut_from_col(&self, c: &mut Col, y: i32, gate: Option<f64>, interior: bool) -> CaveCut {
        let rough = c.get(lane::ROUGH, y);
        let mut shell = false;

        // ENTRANCE ARM — deliberately never caliber-modulated: it is reached
        // from `feature_surface_after_caves`, the hottest cave point query in
        // worldgen, and a biome-dependent mouth would force the biome field
        // into every surface column. A mouth in a caliber-modulating biome
        // still gets that biome's lining BLOCK (the shell arm below runs on a
        // full lattice), just the engine's default shell width.
        if let Some(ease) = gate {
            let metric = c.get(lane::A, y).abs().max(c.get(lane::B, y).abs());
            let base_r = lerp(CAVE_ENTRANCE_SURFACE_R, CAVE_ENTRANCE_DEEP_R, ease);
            let radius = (base_r + rough * CAVE_TUNNEL_ROUGHNESS).max(0.016);
            if metric < radius {
                return CaveCut::Open;
            }
            shell |= metric < radius + CAVE_LINING_SHELL;
        }
        if !interior {
            return if shell {
                CaveCut::Shell
            } else {
                CaveCut::Solid
            };
        }

        // INTERIOR ARM — the only place caliber applies. With no modulating row
        // loaded this is a perfectly predicted branch and the biome field is
        // never touched, so vanilla output and cost are unchanged.
        let cal = if self.caliber_varies {
            self.underground.caliber_at(c.get(lane::BIOME, y), y)
        } else {
            self.underground.base
        };
        let lining_shell = CAVE_LINING_SHELL * cal.shell;
        // A data-declared room widens the radius carvers running through it, so
        // a tunnel arriving at its rim flares OPEN into the room instead of
        // passing five blocks away behind solid rock. Branched rather than
        // multiplied by a 1.0: this is the hottest arithmetic in worldgen and
        // the lane is absent for every table that declares no room.
        let tunnel_gain = if c.lat.chamber_tunnel.is_empty() {
            0.0
        } else {
            c.get(lane::TUNNEL, y)
        };
        let flared = |r: f64| {
            if tunnel_gain == 0.0 {
                r
            } else {
                r * (1.0 + tunnel_gain)
            }
        };

        // Spaghetti: both decorrelated fields near zero -> a long winding tunnel.
        // The thickness modulation spans ~2×..6× of the noodle caliber around a
        // 4× base (see settings.rs).
        let a = c.get(lane::A, y).abs();
        let metric = a.max(c.get(lane::B, y).abs());
        let tunnel_r =
            flared((CAVE_TUNNEL_R + rough * CAVE_TUNNEL_ROUGHNESS).max(0.018) * cal.tunnel);
        if metric < tunnel_r {
            return CaveCut::Open;
        }
        shell |= metric < tunnel_r + lining_shell;

        // Spaghetti branches: a second, slightly tighter tunnel family sharing
        // field A with the main system. Both run along the same A≈0 sheet, so
        // their curves cross at isolated points — junctions where a tunnel
        // forks off the main run.
        let branch_metric = a.max(c.get(lane::BRANCH, y).abs());
        let branch_r = tunnel_r * CAVE_BRANCH_R_SCALE;
        if branch_metric < branch_r {
            return CaveCut::Open;
        }
        shell |= branch_metric < branch_r + lining_shell;

        // Noodle: the same intersection trick at higher frequency and a sliver of
        // a radius — tight 1–2 block crawl spaces, in the LOW-roughness regions
        // (where the spaghetti runs thin, complementing it).
        if rough < CAVE_NOODLE_GATE_T {
            let noodle = c.get(lane::NA, y).abs().max(c.get(lane::NB, y).abs());
            let noodle_r = flared(CAVE_NOODLE_R * cal.tunnel);
            if noodle < noodle_r {
                return CaveCut::Open;
            }
            shell |= noodle < noodle_r + lining_shell;
        }

        // Cheese: a low-frequency field dipping below a depth-scaled threshold ->
        // large caverns, rare near the surface, common near the world floor.
        //
        // A data-declared CHAMBER is one more additive term on this same
        // threshold, which is what makes a mod's room part of the carve rather
        // than a stamp over it: inside the room the threshold clears the
        // sampler's range outright, across the rim the noise still decides, so
        // a tunnel running into it merges instead of ending at a flat face.
        let cheese_t =
            cheese_threshold(y) + cal.cheese + c.chamber(y) + rough * CAVE_CHEESE_ROUGHNESS;
        let cheese = c.get(lane::CHEESE, y);
        if cheese < cheese_t {
            return CaveCut::Open;
        }
        shell |= cheese < cheese_t + CAVE_CHEESE_LINING_SHELL * cal.shell;

        if shell {
            CaveCut::Shell
        } else {
            CaveCut::Solid
        }
    }

    /// One voxel's carve decision through a column cursor — the hot form, so a
    /// walk down a column pays the lattice's x/z interpolation once.
    #[inline]
    fn cut_col(&self, c: &mut Col, x: i32, y: i32, z: i32, surf_y: i32) -> CaveCut {
        if y > surf_y {
            return CaveCut::Solid;
        }
        let gate = self.entrance_gate_ease(x, y, z, surf_y);
        let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
        if gate.is_none() && !interior {
            return CaveCut::Solid;
        }
        self.cut_from_col(c, y, gate, interior)
    }

    #[inline]
    fn cut_lat(&self, lat: &CaveLattice, x: i32, y: i32, z: i32, surf_y: i32) -> CaveCut {
        self.cut_col(&mut Col::new(lat, x, z), x, y, z, surf_y)
    }

    #[cfg(test)]
    fn carved_lat(&self, lat: &CaveLattice, x: i32, y: i32, z: i32, surf_y: i32) -> bool {
        self.cut_lat(lat, x, y, z, surf_y) == CaveCut::Open
    }

    /// Test seam: the rooms an inclusive world box gathers, so the gather's
    /// box-independence can be probed directly rather than through a carve.
    #[cfg(test)]
    pub(super) fn chamber_field(&self, lo: [i32; 3], hi: [i32; 3]) -> chamber::ChamberField {
        chamber::ChamberField::gather(self.underground, self.seed, lo, hi, |x, y, z| {
            self.sample_biome(x as f64, y as f64, z as f64)
        })
    }

    #[cfg(test)]
    pub(super) fn biome_field_sample(&self, p: [i32; 3]) -> f64 {
        self.sample_biome(p[0] as f64, p[1] as f64, p[2] as f64)
    }

    /// Test seam: the field that kneads a room's rim, at the corner the lattice
    /// would read it at.
    #[cfg(test)]
    pub(super) fn knead_sample(&self, x: i32, y: i32, z: i32) -> f64 {
        self.sample_na(x as f64, y as f64, z as f64)
    }

    /// The underground biome owning world `(x,y,z)`, one voxel at a time — the
    /// REFERENCE form the batch path is pinned against.
    #[inline]
    fn biome_id_lat(&self, lat: &CaveLattice, x: i32, y: i32, z: i32) -> u8 {
        self.underground.id_at(lat.biome(x, y, z), y)
    }

    /// The underground biome owning world `(x,y,z)` — the SAME decision the
    /// carver's lining and caliber read, through the same world-anchored
    /// lattice, so a mod placing content inside a biome gets that biome's
    /// caves. Sampling the field exactly here instead would disagree near band
    /// boundaries: the carver reads a trilinear value on the lattice grid.
    #[cfg(test)]
    pub fn underground_biome_at(&self, x: i32, y: i32, z: i32) -> u8 {
        let fields = Fields {
            carve: false,
            interior: false,
            biome: true,
        };
        let lat = self.build_lattice_filtered(x, y, z, x, y, z, fields);
        self.biome_id_lat(&lat, x, y, z)
    }

    /// Post-cave top non-air surface for a land column, before vegetation/trees.
    ///
    /// Most columns return `surf_y` without scanning. Only when the entrance field
    /// actually cuts the surface do we walk down until the first non-carved voxel,
    /// matching the later block carve.
    pub fn surface_after_caves(&self, x: i32, z: i32, surf_y: i32) -> i32 {
        if !self.cave_carved(x, surf_y, z, surf_y) {
            return surf_y;
        }
        let mut y = surf_y;
        while y >= CAVE_MIN_Y && self.cave_carved(x, y, z, surf_y) {
            y -= 1;
        }
        y
    }

    /// Surface used only for tree/feature anchoring. Cave-mouth columns are
    /// deliberately treated as unsuitable roots so generated trunks do not plug
    /// entrances. A column is a mouth iff its surface voxel is carved, so this
    /// never pays for the downward walk [`surface_after_caves`] does — it runs
    /// per cell over the padded feature windows, the hottest cave point-query.
    pub fn feature_surface_after_caves(&self, x: i32, z: i32, surf_y: i32) -> i32 {
        if self.cave_carved(x, surf_y, z, surf_y) {
            CAVE_ENTRANCE_MIN_SURFACE_Y
                .min(surf_y)
                .min(crate::chunk::SEA_LEVEL)
        } else {
            surf_y
        }
    }

    /// Conservative generated-summary helper. If this returns true the section may
    /// contain cave air, so callers must not claim it is virtual full stone.
    pub fn section_may_carve(cy: i32, surf_min: i32, surf_max: i32) -> bool {
        let y0 = cy * SECTION_SIZE as i32;
        let y1 = y0 + SECTION_SIZE as i32 - 1;
        if y0 > surf_max || y1 < CAVE_MIN_Y {
            return false;
        }

        let interior = y0 <= surf_max - CAVE_SURFACE_BUFFER;
        let entrance = surf_max >= CAVE_ENTRANCE_MIN_SURFACE_Y
            && y0 <= surf_max
            && y1 >= surf_min - CAVE_ENTRANCE_MAX_DEPTH;
        interior || entrance
    }

    /// Lowest Y a batch carve touches. Normally the carve floor, but a whole
    /// course under it when some row guarantees a FLOOR lining: the deepest
    /// cave in the world bottoms out on the plane the carvers refuse to cut,
    /// and that plane is the one surface the shell arm structurally cannot
    /// reach (it needs `interior`, and `interior` is what stops the carve
    /// there). A guarantee with a hole in its most walkable plane is not a
    /// guarantee, and neither is a course that thins out there.
    #[inline]
    fn batch_min_y(&self) -> i32 {
        if self.underground.lining_floor_under_world_floor {
            CAVE_MIN_Y - self.underground.lining_floor_depth_max.max(1)
        } else {
            CAVE_MIN_Y
        }
    }

    /// Voxels the lattice must carry ABOVE the box's top: a floor course can
    /// start above it and reach down into it, and the batch measures that by
    /// probing rather than remembering.
    #[inline]
    fn batch_y_pad(&self) -> i32 {
        if self.lining_faces {
            self.underground.lining_floor_depth_max.max(1)
        } else {
            0
        }
    }

    /// Voxels the lattice must carry BELOW the box's floor. A section floor is
    /// not a world floor: the cell under it is what decides whether the box's
    /// lowest rock is a CEILING, and only the carve field knows — the block
    /// itself lives in a section this batch cannot see. Zero once the box
    /// bottoms out on the lowest plane the carve reaches, where nothing below
    /// can be open.
    #[inline]
    fn batch_y_pad_low(&self, y0: i32) -> i32 {
        (self.lining_faces && y0 > self.batch_min_y()) as i32
    }

    pub fn carve_chunk(&self, chunk: &mut Chunk, surf: &[i32]) {
        debug_assert_eq!(surf.len(), CHUNK_SX * CHUNK_SZ);
        let (ox, oz) = chunk.chunk_origin_world();
        let mut carved = false;

        let y0 = self.batch_min_y().max(0);
        let y1 = surf
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .min(CHUNK_SY as i32 - 1);
        if y0 > y1 {
            return;
        }
        let lat = self.build_lattice(
            ox,
            y0 - self.batch_y_pad_low(y0),
            oz,
            ox + CHUNK_SX as i32 - 1,
            y1 + self.batch_y_pad(),
            oz + CHUNK_SZ as i32 - 1,
        );
        let batch = BatchCarve::new(self, &lat);
        let blocks = chunk.blocks_slice_mut();

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let surf_y = surf[z * CHUNK_SX + x];
                let y1 = surf_y.min(CHUNK_SY as i32 - 1);
                if y0 > y1 {
                    continue;
                }
                let slot = |y| idx(x, y as usize, z);
                let (wx, wz) = (ox + x as i32, oz + z as i32);
                carved |= if batch.faces {
                    batch.column::<true, _>(blocks, slot, wx, wz, y0, y1, surf_y)
                } else {
                    batch.column::<false, _>(blocks, slot, wx, wz, y0, y1, surf_y)
                };
            }
        }

        if carved {
            chunk.recompute_heightmap();
            chunk.recompute_random_tick_count();
        }
    }

    pub fn carve_section(&self, section: &mut Section, surf: &[i32]) {
        debug_assert_eq!(surf.len(), SECTION_SIZE * SECTION_SIZE);
        let (ox, oy, oz) = section.origin_world();

        let y0 = oy.max(self.batch_min_y());
        let y1 = (oy + SECTION_SIZE as i32 - 1).min(surf.iter().copied().max().unwrap_or(i32::MIN));
        if y0 > y1 {
            return;
        }
        let lat = self.build_lattice(
            ox,
            y0 - self.batch_y_pad_low(y0),
            oz,
            ox + SECTION_SIZE as i32 - 1,
            y1 + self.batch_y_pad(),
            oz + SECTION_SIZE as i32 - 1,
        );
        let batch = BatchCarve::new(self, &lat);
        section.edit_ids_bulk(|blocks| {
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let surf_y = surf[z * SECTION_SIZE + x];
                    let y1 = (oy + SECTION_SIZE as i32 - 1).min(surf_y);
                    if y0 > y1 {
                        continue;
                    }
                    let slot = |y| section_idx(x, (y - oy) as usize, z);
                    let (wx, wz) = (ox + x as i32, oz + z as i32);
                    if batch.faces {
                        batch.column::<true, _>(blocks, slot, wx, wz, y0, y1, surf_y);
                    } else {
                        batch.column::<false, _>(blocks, slot, wx, wz, y0, y1, surf_y);
                    }
                }
            }
        });
    }

    /// The entrance GATE: exactly-sampled (hot per-column in `ColumnGen`, where a
    /// lattice would cost more than the two samples it replaces). Returns the depth
    /// ease for the radius test when the gate opens, `None` otherwise.
    #[inline]
    fn entrance_gate_ease(&self, x: i32, y: i32, z: i32, surf_y: i32) -> Option<f64> {
        if surf_y < CAVE_ENTRANCE_MIN_SURFACE_Y {
            return None;
        }
        let depth = surf_y - y;
        if !(0..=CAVE_ENTRANCE_MAX_DEPTH).contains(&depth) {
            return None;
        }

        let t = depth as f64 / CAVE_ENTRANCE_MAX_DEPTH as f64;
        let ease = smoothstep(t);
        let threshold = lerp(
            CAVE_ENTRANCE_GATE_SURFACE_T,
            CAVE_ENTRANCE_GATE_DEEP_T,
            ease,
        );

        let (fx, fy, fz) = (x as f64, y as f64, z as f64);
        let gate = self.entrance_a.get([
            fx * CAVE_ENTRANCE_FREQ,
            fy * CAVE_ENTRANCE_FREQ * CAVE_ENTRANCE_Y_SCALE,
            fz * CAVE_ENTRANCE_FREQ,
        ]) + 0.35
            * self.entrance_b.get([
                fx * CAVE_ENTRANCE_FREQ * 1.7 + 37.1,
                fy * CAVE_ENTRANCE_FREQ * CAVE_ENTRANCE_Y_SCALE * 1.3 + 11.3,
                fz * CAVE_ENTRANCE_FREQ * 1.7 - 19.7,
            ]);
        (gate <= threshold).then_some(ease)
    }
}

/// Rock cells below a cave floor a row may paint. Capped at one lattice step so
/// the skip mask's one-cell dilation still covers every cell the rule reaches.
const MAX_FLOOR_DEPTH: usize = LATTICE_STEP as usize;
const _: () = assert!(
    MAX_FLOOR_DEPTH <= LATTICE_STEP as usize,
    "a deeper course than one lattice cell needs a deeper mask dilation"
);

/// A course slot whose cell was not stone when the walk reached it. It still
/// OCCUPIES its depth — a course's reach is geometry, not block ids, because
/// only the batch owning a cell can see its id and a course crosses batches.
const NOT_STONE: usize = usize::MAX;

/// One batch carve's shared state. Both batch paths — whole column and cubic
/// section — drive the SAME column walk through this, because the orientation
/// lining is loop-shaped (it reads what the cell above turned out to be) and
/// two copies of that would be free to disagree below y=0, where the
/// chunk/section parity test does not look.
///
/// Sharing the walk is not enough on its own: its carry has to be seeded and
/// flushed by ASKING the carve field, never by assuming the box floor is a
/// world floor. Both ends of a column are box boundaries for exactly one of
/// the two paths, so anything remembered across a voxel is container-shaped
/// state unless the other end can re-derive it.
struct BatchCarve<'a> {
    field: &'a CaveField,
    lat: &'a CaveLattice,
    may_cut: Vec<bool>,
    mx: usize,
    mz: usize,
    /// Hoisted: does any loaded row line per orientation? Everything the
    /// orientation rule needs — the run bookkeeping, the top-of-column probe,
    /// the biome read on a SOLID cell — hangs off this, so a table without it
    /// walks exactly the loop it always did.
    faces: bool,
    air: u16,
    water: u16,
    stone: u16,
}

impl<'a> BatchCarve<'a> {
    fn new(field: &'a CaveField, lat: &'a CaveLattice) -> Self {
        Self {
            field,
            lat,
            may_cut: lat.may_cut_mask(field.underground, field.lining_faces),
            mx: lat.nx - 1,
            mz: lat.nz - 1,
            faces: field.lining_faces,
            air: Block::Air.id(),
            water: Block::Water.id(),
            stone: Block::Stone.id(),
        }
    }

    /// Carve and line the inclusive column `y0..=y1`. `slot` maps a world Y to
    /// the caller's buffer index. Returns whether anything was carved.
    ///
    /// The walk ASCENDS, which is what makes the floor rule free: a cave floor
    /// is known one step after it is written, so it is repainted by index
    /// rather than discovered by probing `y+1` at every solid voxel — which
    /// would double the carve decisions in the hottest loop in worldgen. Only
    /// the column's two ENDS need real probes, because that is exactly where
    /// the neighbouring voxel belongs to another batch.
    fn column<const FACES: bool, F: Fn(i32) -> usize>(
        &self,
        blocks: &mut [u16],
        slot: F,
        wx: i32,
        wz: i32,
        y0: i32,
        y1: i32,
        surf_y: i32,
    ) -> bool {
        let (air, water, stone) = (self.air, self.water, self.stone);
        debug_assert_eq!(FACES, self.faces);
        let cxc = (wx.div_euclid(LATTICE_STEP) - self.lat.lx0) as usize;
        let czc = (wz.div_euclid(LATTICE_STEP) - self.lat.lz0) as usize;
        let col = czc * self.mx + cxc;
        // One cursor for the whole walk: x and z are fixed, so every lane's
        // xz interpolation is computed once per lattice cell instead of once
        // per lane per voxel.
        let mut cur = Col::new(self.lat, wx, wz);
        let stride = self.mz * self.mx;
        let ly0 = self.lat.ly0;
        let mut carved = false;
        // The contiguous run of solid cells immediately below the cursor, in
        // ascending Y — what a cave floor opening at the cursor paints.
        let mut run = [(0i32, NOT_STONE); MAX_FLOOR_DEPTH];
        let mut run_len = 0usize;
        // Seeded, not assumed: the voxel under the box floor is the other
        // batch's, so whether the box's lowest rock is a CEILING is a question
        // for the carve field. Guessing `false` here is one whole voxel plane
        // per section taking the WALL rule.
        let mut below_open = FACES
            && self.field.batch_y_pad_low(y0) != 0
            && self.field.cut_col(&mut cur, wx, y0 - 1, wz, surf_y) == CaveCut::Open;
        let mut y = y0;
        while y <= y1 {
            let cyc = (y.div_euclid(LATTICE_STEP) - ly0) as usize;
            if !self.may_cut[cyc * stride + col] {
                // Provably solid cell: jump to the next cell floor.
                y = (y.div_euclid(LATTICE_STEP) + 1) * LATTICE_STEP;
                if FACES {
                    run_len = 0;
                    below_open = false;
                }
                continue;
            }
            let i = slot(y);
            let id = blocks[i];
            if id == air || id == water {
                y += 1;
                if FACES {
                    run_len = 0;
                    below_open = false;
                }
                continue;
            }
            match self.field.cut_col(&mut cur, wx, y, wz, surf_y) {
                CaveCut::Open => {
                    blocks[i] = air;
                    carved = true;
                    if FACES {
                        self.paint_floor(blocks, &run[..run_len], wx, wz);
                        run_len = 0;
                        below_open = true;
                    }
                }
                cut => {
                    if cut == CaveCut::Shell && id == stone {
                        self.paint_side::<FACES>(blocks, i, below_open, wx, y, wz);
                    }
                    if FACES {
                        if run_len == MAX_FLOOR_DEPTH {
                            run.copy_within(1.., 0);
                            run_len -= 1;
                        }
                        run[run_len] = (y, if id == stone { i } else { NOT_STONE });
                        run_len += 1;
                        below_open = false;
                    }
                }
            }
            y += 1;
        }
        // The column ended on rock. Whether that rock is a cave FLOOR, and how
        // far below the floor it sits, is the next batch's business — so this
        // is the other place the rule has to ask rather than remember, and the
        // lattice was padded by the deepest declared course for it.
        if FACES && run_len > 0 {
            self.paint_floor_below_top(blocks, &run[..run_len], wx, wz, y1, surf_y);
        }
        carved
    }

    /// The floor rule owning a course whose TOP cell is `top_y`. The whole
    /// course resolves against that one cell: it is the only cell BOTH batches
    /// sharing a course can name, so resolving per cell — or against whichever
    /// end of the course happened to fall inside this box — would hand the two
    /// paths different rows wherever a course crosses a band edge.
    #[inline]
    fn floor_rule(&self, top_y: i32, wx: i32, wz: i32) -> Option<&'static LiningFaces> {
        let id = self.field.biome_id_lat(self.lat, wx, top_y, wz);
        let f = self.field.underground.faces(id)?;
        (f.floor.block != self.air).then_some(f)
    }

    /// Paint the top `take` cells of a course. Slots the walk saw as non-stone
    /// are skipped but still spent, so how deep the course reaches does not
    /// depend on what the rock happened to be made of above this box.
    ///
    /// `depth0` is the COURSE depth of the run's top cell — `0` when this run
    /// starts at the course top, and the number of cells already spent above
    /// this box otherwise. It is what tells a `floor_under` layering which
    /// cell is the surface, and it must come from the caller: a run that
    /// begins mid-course cannot see its own top, and guessing `0` would paint
    /// a second surface on every box boundary.
    #[inline]
    fn paint_run(
        &self,
        blocks: &mut [u16],
        run: &[(i32, usize)],
        f: &LiningFaces,
        wx: i32,
        wz: i32,
        take: usize,
        depth0: i32,
    ) {
        for (k, &(y, i)) in run.iter().rev().take(take).enumerate() {
            let lining = match f.floor_under {
                Some(under) if depth0 + k as i32 > 0 => under,
                _ => f.floor,
            };
            if i != NOT_STONE && face_roll(self.field.seed, f.salt, lining.weight, wx, y, wz) {
                blocks[i] = lining.block;
            }
        }
    }

    /// Paint the rock under a cave floor the walk just cut: the run's top cell
    /// IS the course top.
    #[inline]
    fn paint_floor(&self, blocks: &mut [u16], run: &[(i32, usize)], wx: i32, wz: i32) {
        let Some(&(top_y, _)) = run.last() else {
            return;
        };
        let Some(f) = self.floor_rule(top_y, wx, wz) else {
            return;
        };
        self.paint_run(blocks, run, f, wx, wz, f.floor_depth as usize, 0);
    }

    /// Paint a run left at the box's top voxel. The cave floor that owns it can
    /// be up to a full course above the box, so both the floor's existence and
    /// the run's DEPTH under it are probed — and the rule is resolved at the
    /// course's real top cell, which is what the batch containing that cell
    /// resolves against too.
    fn paint_floor_below_top(
        &self,
        blocks: &mut [u16],
        run: &[(i32, usize)],
        wx: i32,
        wz: i32,
        y1: i32,
        surf_y: i32,
    ) {
        for above in 0..self.field.batch_y_pad() {
            if self.field.cut_lat(self.lat, wx, y1 + 1 + above, wz, surf_y) != CaveCut::Open {
                continue;
            }
            let Some(f) = self.floor_rule(y1 + above, wx, wz) else {
                return;
            };
            let reach = f.floor_depth - above;
            if reach > 0 {
                self.paint_run(blocks, run, f, wx, wz, reach as usize, above);
            }
            return;
        }
    }

    /// Paint a cell that hugs a cave wall: the CEILING rule when the voxel
    /// below it was carved, the WALL rule otherwise. A floor repaints over this
    /// on the next step, which is the precedence a floor GUARANTEE needs.
    #[inline]
    fn paint_side<const FACES: bool>(
        &self,
        blocks: &mut [u16],
        i: usize,
        below_open: bool,
        wx: i32,
        y: i32,
        wz: i32,
    ) {
        let id = self.field.biome_id_lat(self.lat, wx, y, wz);
        let bare = |blocks: &mut [u16]| {
            let lining = self.field.underground.lining(id);
            if lining != self.air {
                blocks[i] = lining;
            }
        };
        // A table with no orientation rule never reads the (dense, and two
        // orders of magnitude larger) per-orientation array at all.
        if !FACES {
            return bare(blocks);
        }
        let Some(f) = self.field.underground.faces(id) else {
            return bare(blocks);
        };
        let face = if below_open { f.ceiling } else { f.wall };
        if face.block != self.air && face_roll(self.field.seed, f.salt, face.weight, wx, y, wz) {
            blocks[i] = face.block;
        }
    }
}

/// Positional dither for a partial-coverage face rule. A weight of 1 takes NO
/// draw at all — that is what separates a guarantee from a 0.99.
#[inline]
fn face_roll(seed: u32, salt: u64, weight: f32, x: i32, y: i32, z: i32) -> bool {
    if weight >= 1.0 {
        return true;
    }
    weight > 0.0 && crate::worldgen::rng::FeatureRng::positional(seed, salt, x, y, z).chance(weight)
}

/// Depth-scaled cheese carve threshold: `CAVE_CHEESE_T_SHALLOW` at/above
/// `CAVE_CHEESE_DEPTH_TOP`, easing to `CAVE_CHEESE_T_DEEP` at/below
/// `CAVE_CHEESE_DEPTH_BOTTOM` — caverns grow bigger and more common with depth.
#[inline]
fn cheese_threshold(y: i32) -> f64 {
    let t = (CAVE_CHEESE_DEPTH_TOP - y) as f64
        / (CAVE_CHEESE_DEPTH_TOP - CAVE_CHEESE_DEPTH_BOTTOM) as f64;
    lerp(CAVE_CHEESE_T_SHALLOW, CAVE_CHEESE_T_DEEP, smoothstep(t))
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[inline]
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::data::underground;

    /// The batched positional queries share ONE lattice across a whole box and
    /// walk a column through a single cursor; both are pure scheduling, so they
    /// must answer exactly what the one-voxel lattice answers, position for
    /// position, whatever the batch happens to contain. A drift here is
    /// invisible — a mod's structures shift by a cell somewhere deep — so it is
    /// pinned against the reference path directly, including a caliber pack
    /// (which arms the biome lane) and singleton batches.
    #[test]
    fn batched_positional_queries_match_the_one_voxel_reference() {
        for pack in [None, Some(CALIBER_PACK)] {
            let table = match pack {
                None => underground::table(),
                Some(p) => underground::test_table(&[p]),
            };
            let field = CaveField::with_table(0x5EED_BEEF, table);
            let mut positions = Vec::new();
            let mut st = 0x9E37_79B9_7F4A_7C15u64;
            for _ in 0..3000 {
                let mut next = |lo: i32, hi: i32| {
                    st ^= st << 13;
                    st ^= st >> 7;
                    st ^= st << 17;
                    lo + (st % ((hi - lo + 1) as u64)) as i32
                };
                positions.push([next(-300, 300), next(CAVE_MIN_Y, 90), next(-300, 300)]);
            }
            // Plus a dense column and a dense slab, the two shapes the
            // subdivision is supposed to collapse into one lattice.
            for y in CAVE_MIN_Y..CAVE_MIN_Y + 60 {
                positions.push([7, y, -13]);
            }
            for x in 0..24 {
                for z in 0..24 {
                    positions.push([x, -20, z]);
                }
            }

            let mut biomes = Vec::new();
            field.underground_biome_at_batch(&positions, &mut biomes);
            let queries: Vec<([i32; 3], i32)> = positions.iter().map(|&p| (p, 70)).collect();
            let mut carved = Vec::new();
            field.cave_carved_batch(&queries, &mut carved);

            for (i, &[x, y, z]) in positions.iter().enumerate() {
                assert_eq!(
                    biomes[i],
                    field.underground_biome_at(x, y, z),
                    "biome batch diverged at {x},{y},{z}"
                );
                assert_eq!(
                    carved[i],
                    field.cave_carved(x, y, z, 70),
                    "carve batch diverged at {x},{y},{z}"
                );
            }
        }
    }

    /// A pack layer whose rows modulate caliber across the WHOLE realizable
    /// field: `mymod:roomy` claims everything the (narrower, so higher
    /// specificity) `mymod:veined` row does not. Every caliber-sensitive
    /// invariant is re-run against this so the knobs are covered, not just
    /// their defaults.
    const CALIBER_PACK: &str = r#"{"underground_biomes": [
        {"underground_biome": "mymod:veined", "field": [0.17, 1.0],
         "lining": {"block": "petramond:marble", "shell": 2.0},
         "caliber": {"tunnel": 2.5, "cheese": 0.2, "blend": [0.02, 4]}},
        {"underground_biome": "mymod:roomy", "field": [-1.5, 0.17],
         "lining": {"block": "petramond:moss_block", "shell": 2.0,
                    "faces": {"ceiling": {"weight": 0.0}}},
         "caliber": {"tunnel": 2.5, "cheese": 0.2, "blend": [0.02, 4]}}]}"#;

    /// Caliber rows banded AWAY from most of the volume: one confined by depth,
    /// one by field. The skip mask narrows its caliber bound per lattice cell,
    /// so this is the table where narrowing too hard shows up — as a cave the
    /// mask skipped and the carver would have opened.
    const BANDED_CALIBER_PACK: &str = r#"{"underground_biomes": [
        {"underground_biome": "deep:roomy", "field": [-1.5, 1.5], "y": [-64, -33],
         "lining": {"block": "petramond:moss_block", "shell": 2.0},
         "caliber": {"tunnel": 2.5, "cheese": 0.2, "blend": [0.02, 4]}},
        {"underground_biome": "rare:roomy", "field": [0.24, 1.5],
         "lining": {"block": "petramond:marble", "shell": 2.0},
         "caliber": {"tunnel": 2.5, "cheese": 0.2, "blend": [0.02, 4]}}]}"#;

    /// A row declaring a CHAMBER: a term summed straight into the cavern carve
    /// threshold, so it arms a lattice lane the shipped table never allocates
    /// and widens the skip mask's cheese bound. Every carve invariant re-runs
    /// against it for exactly that reason — it is the newest way for the point
    /// and batch paths, and for the mask and the carver, to drift apart.
    const CHAMBER_PACK: &str = r#"{"underground_biomes": [
        {"underground_biome": "wide:rock", "field": [-1.5, 0.24]},
        {"underground_biome": "mymod:cathedral", "field": [0.24, 1.0], "y": [-64, -16],
         "lining": {"block": "petramond:moss_block", "shell": 1.4,
                    "faces": {"floor_depth": 2,
                              "floor": {"block": "petramond:moss_block"},
                              "wall": {"weight": 0.7},
                              "ceiling": {"weight": 0.2}}},
         "caliber": {"tunnel": 1.6, "cheese": 0.22, "blend": [0.02, 4]},
         "chamber": {"lattice": 64, "one_in": 1, "radius": [10, 14],
                     "flatten": 0.45, "sill": 0.75, "feather": 9, "strength": 1.0,
                     "lobes": 3, "lobe_spread": 0.7, "lobe_scale": [0.55, 0.8],
                     "tunnel": 2.0, "rim_noise": 1.5}}]}"#;

    /// A chamber row banded to a rare field value AND a narrow depth, so most
    /// of any test box lies outside the lane's declared span. That is where a
    /// gate on the lane can be wrong in the dangerous direction — skipping it
    /// somewhere a room does reach.
    const BANDED_CHAMBER_PACK: &str = r#"{"underground_biomes": [
        {"underground_biome": "wide:rock", "field": [-1.5, 0.22]},
        {"underground_biome": "deep:cathedral", "field": [0.22, 1.0], "y": [-64, -24],
         "lining": {"block": "petramond:moss_block"},
         "chamber": {"lattice": 32, "one_in": 1, "radius": [5, 6],
                     "flatten": 0.9, "sill": 0.4, "feather": 8, "strength": 1.6}}]}"#;

    /// The shipped table, a caliber-modulating one, one whose caliber rows are
    /// banded away from most of the volume, and two declaring CHAMBERS — every
    /// carve invariant must hold for all of them, since a pack turning the
    /// biome field into a carve INPUT is exactly what re-arms the paths that
    /// can silently drift.
    /// Does a table declare anything beyond the structural FALLBACK? Since
    /// marble became ordinary stone-hosted rock (2026-07-27) the engine ships
    /// exactly ONE underground biome, so `test_table(&[])` is a single-biome
    /// world — a "this sample spans more than one biome" guard against it would
    /// be checking the fixture, not the code.
    ///
    /// The rarely-banded pack fixtures used to inherit a second biome for free
    /// from marble's wide `[0.17, 1.0]` band; they now declare a `wide:rock`
    /// row for it. It carries no caliber, lining or chamber, so it changes
    /// which id a cell REPORTS and nothing the carve does.
    fn has_banded_rows(table: &underground::UndergroundBiomes) -> bool {
        table.name(1).is_some()
    }

    fn tables() -> [&'static underground::UndergroundBiomes; 5] {
        [
            underground::test_table(&[]),
            underground::test_table(&[CALIBER_PACK]),
            underground::test_table(&[BANDED_CALIBER_PACK]),
            underground::test_table(&[CHAMBER_PACK]),
            underground::test_table(&[BANDED_CHAMBER_PACK]),
        ]
    }

    /// Boxes worth sweeping for a table: a fixed scatter, plus — when the table
    /// declares chambers — boxes straddling the RIM of every room the seed
    /// actually rolls nearby. The rim is where a room's term is neither zero
    /// nor saturated, i.e. the only place a bound or a lane gate can be wrong
    /// in a way that still produces plausible output.
    fn sweep_boxes(field: &CaveField, span: i32) -> Vec<[i32; 3]> {
        let mut boxes = vec![
            [-8, -40, 24],
            [-20, -48, 12],
            [96, -60, -144],
            [-232, -24, 88],
            [312, 8, 296],
        ];
        if field.chamber_y_span.is_some() {
            let rooms = field.chamber_field([-768, -64, -768], [768, -8, 768]);
            // A handful is enough; every extra room multiplies a cubic sweep.
            for c in rooms.centers().into_iter().take(3) {
                // One box on the room's core, and four crossing its rim from
                // different sides, so both the saturated and the dissolving
                // part of the term are swept.
                boxes.push([c[0] - span / 2, c[1] - span / 2, c[2] - span / 2]);
                for (dx, dy, dz) in [(18, 0, 0), (-24, 0, 6), (0, 9, -20), (6, -11, 16)] {
                    boxes.push([c[0] + dx - span / 2, c[1] + dy, c[2] + dz - span / 2]);
                }
            }
            assert!(
                boxes.len() > 5,
                "no chamber rolled anywhere near the sweep; the coverage is vacuous"
            );
        }
        boxes
    }

    /// The invariant everything hangs on: the point path (surface walks feeding
    /// column heightmaps) and the batch path (the actual block carve) must agree at
    /// every voxel, or heightmaps drift from carved blocks and skylight breaks.
    ///
    /// With a caliber table this is also the hazard the sparse path is most
    /// prone to: the point query builds a PARTIAL lattice, so a producer that
    /// skips a field the consumer reads shows up here as a wrong answer (and in
    /// debug as the named lattice assertion) long before it can become a bounds
    /// panic in production. A CHAMBER lane raises the stakes: it is skipped for
    /// whole boxes, so producer and consumer must agree not only about how to
    /// read it but about when it is provably zero.
    #[test]
    fn point_and_batch_carve_decisions_agree() {
        const SPAN: i32 = 19;
        for table in tables() {
            for seed in [0x51EEDu32, 0x1D001, 0x26AAC] {
                let field = CaveField::with_table(seed, table);
                let (mut carved, mut chamber_boxes) = (0usize, 0usize);
                for [x0, y0, z0] in sweep_boxes(&field, SPAN) {
                    let (x1, y1, z1) = (x0 + SPAN, y0 + SPAN, z0 + SPAN);
                    let lat = field.build_lattice(x0, y0, z0, x1, y1, z1);
                    chamber_boxes += lat.chamber_is_live() as usize;
                    for surf_y in [y1 - 2, y1 + 80] {
                        for y in y0..=y1 {
                            for z in z0..=z1 {
                                for x in x0..=x1 {
                                    let batch = field.carved_lat(&lat, x, y, z, surf_y);
                                    let point = field.cave_carved(x, y, z, surf_y);
                                    assert_eq!(
                                        batch, point,
                                        "divergence at ({x},{y},{z}) surf {surf_y} seed {seed:#x}"
                                    );
                                    carved += batch as usize;
                                }
                            }
                        }
                    }
                }
                // Not shape pins — proof the sweep exercised what it claims to.
                assert!(carved > 0, "test volume should contain some cave air");
                assert!(
                    table.chamber_y_span.is_none() || chamber_boxes > 0,
                    "no swept box carried a live chamber lane (seed {seed:#x})"
                );
            }
        }
    }

    /// The skip mask may only drop cells it can PROVE are all-solid. Nothing
    /// else asserts this, and a mask that forgot to widen by a pack's caliber —
    /// or by the chamber term now riding the same cavern threshold — deletes
    /// caves while still producing plausible-looking output.
    ///
    /// Adversarial on purpose: several seeds, and boxes aimed at the rims of
    /// the rooms each seed actually rolls, since a chamber's bound is exact in
    /// its core and only interesting where the term is partial.
    ///
    /// A skipped cell must also not be a cell the LINING would have painted. An
    /// orientation lining paints the rock UNDER carved air even when that rock
    /// is outside the shell band, and that rock can sit in the lattice cell
    /// below the air's — so the mask has to be dilated. Skipping it produces
    /// moss that appears or not depending on where the 4-block boundary falls,
    /// which is plausible output with nothing asserting against it.
    #[test]
    fn may_cut_mask_never_skips_a_carved_cell() {
        const SPAN: i32 = 23;
        for table in tables() {
            for seed in [0xA5C0u32, 0x312, 0x1D001, 0x2BEEF] {
                let field = CaveField::with_table(seed, table);
                let (mut skipped, mut chamber_boxes) = (0usize, 0usize);
                for [x0, y0, z0] in sweep_boxes(&field, SPAN) {
                    let (x1, y1, z1) = (x0 + SPAN, y0 + SPAN, z0 + SPAN);
                    let lat = field.build_lattice(x0, y0, z0, x1, y1 + MAX_FLOOR_DEPTH as i32, z1);
                    chamber_boxes += lat.chamber_is_live() as usize;
                    let mask = lat.may_cut_mask(field.underground, field.lining_faces);
                    let (mx, mz) = (lat.nx - 1, lat.nz - 1);
                    for surf_y in [y1 - 2, y1 + 80] {
                        for y in y0..=y1 {
                            for z in z0..=z1 {
                                for x in x0..=x1 {
                                    let cx = (x.div_euclid(LATTICE_STEP) - lat.lx0) as usize;
                                    let cy = (y.div_euclid(LATTICE_STEP) - lat.ly0) as usize;
                                    let cz = (z.div_euclid(LATTICE_STEP) - lat.lz0) as usize;
                                    if mask[(cy * mz + cz) * mx + cx] {
                                        continue;
                                    }
                                    skipped += 1;
                                    assert_eq!(
                                        field.cut_lat(&lat, x, y, z, surf_y),
                                        CaveCut::Solid,
                                        "mask skipped ({x},{y},{z}) surf {surf_y} seed {seed:#x} \
                                         but the carver acts"
                                    );
                                    // The bound the ONE-cell dilation actually
                                    // rests on: a course reaches
                                    // `MAX_FLOOR_DEPTH` down, so a skipped
                                    // cell must have no carved air that far
                                    // above it either. Asserting only `y+1`
                                    // would keep passing if the course ever
                                    // outgrew the dilation.
                                    if field.lining_faces {
                                        for d in 1..=MAX_FLOOR_DEPTH as i32 {
                                            assert_ne!(
                                                field.cut_lat(&lat, x, y + d, z, surf_y),
                                                CaveCut::Open,
                                                "mask skipped ({x},{y},{z}) surf {surf_y} seed \
                                                 {seed:#x} but a cave FLOOR course {d} above \
                                                 reaches it"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                assert!(skipped > 0, "the mask must actually skip something here");
                assert!(
                    table.chamber_y_span.is_none() || chamber_boxes > 0,
                    "no swept box carried a live chamber lane (seed {seed:#x})"
                );
            }
        }
    }

    /// A chamber lane is SKIPPED for boxes no declared room can reach, which is
    /// only sound if the term there is provably zero. Probe the depth span from
    /// both sides: just inside it the lane must be live somewhere, and just
    /// outside it every gathered room must contribute exactly nothing — a lane
    /// gate that is one block too tight deletes the top or bottom slice of
    /// every room in the world and nothing downstream can tell.
    #[test]
    fn the_chamber_lane_is_only_skipped_where_no_room_can_reach() {
        for table in tables() {
            let Some((lo, hi)) = table.chamber_y_span else {
                continue;
            };
            let field = CaveField::with_table(0x1D001, table);
            // Outside the span the gather itself must come back empty over a
            // wide box, whatever the field says.
            for y in [lo - 1, lo - 64, hi + 1, hi + 64] {
                let rooms = field.chamber_field([-512, y, -512], [512, y, 512]);
                for c in rooms.centers() {
                    let far = field.chamber_field([c[0], y, c[2]], [c[0], y, c[2]]);
                    assert_eq!(
                        far.at(c[0], y, c[2], field.knead_sample(c[0], y, c[2])),
                        (0.0, 0.0),
                        "a room contributes at y={y}, outside the declared span {lo}..={hi}"
                    );
                }
            }
            // Inside it, the lane must be live for at least one real box, or
            // the span is so loose that skipping never happens.
            let live = sweep_boxes(&field, 19).into_iter().any(|[x, y, z]| {
                field
                    .build_lattice(x, y, z, x + 19, y + 19, z + 19)
                    .chamber_is_live()
            });
            assert!(live, "the declared span never yields a live lane");
        }
    }

    /// The other half of the skip mask's contract: it widens by the caliber
    /// that can reach THIS cell, not by the widest any loaded row declares. A
    /// row banded to a depth or a field value a box never touches must cost
    /// that box nothing — otherwise one pack row taxes worldgen everywhere,
    /// the bound converges on the load caps as packs accumulate, and the mask
    /// stops skipping anything for anyone.
    #[test]
    fn a_row_banded_away_from_a_box_does_not_widen_its_skip_mask() {
        // Maximum caliber, confined to the bottom of the world.
        const DEEP: &str = r#"{"underground_biomes": [
            {"underground_biome": "deep:roomy", "field": [-1.5, 1.5], "y": [-64, -49],
             "lining": {"block": "petramond:moss_block", "shell": 4.0},
             "caliber": {"tunnel": 4.0, "cheese": 0.5, "blend": [0, 0]}}]}"#;
        let plain = underground::test_table(&[]);
        let banded = underground::test_table(&[DEEP]);
        // Both tables read the same fields, so one sampler serves both masks.
        let field = CaveField::with_table(0xA5C0, plain);

        let outside = field.build_lattice(-20, 0, 12, 11, 31, 43);
        assert_eq!(
            outside.may_cut_mask(banded, false),
            outside.may_cut_mask(plain, false),
            "a row that cannot reach this box must not widen its skip bounds"
        );

        let inside = field.build_lattice(-20, -64, 12, 11, -49, 43);
        let (wide, narrow) = (
            inside.may_cut_mask(banded, false),
            inside.may_cut_mask(plain, false),
        );
        assert!(
            narrow.iter().any(|c| !c),
            "the box must be one the mask can skip in, or this proves nothing"
        );
        assert!(
            wide.iter().zip(&narrow).all(|(w, n)| *w || !n),
            "widening may only ADD cells"
        );
        assert!(
            wide.iter().filter(|c| **c).count() > narrow.iter().filter(|c| **c).count(),
            "inside its own band the row's caliber must still widen the mask"
        );
    }

    /// The mod ABI's underground-biome query must answer exactly what the
    /// carver's lining and caliber read at that cell — the contract a pack's
    /// content placement depends on ("place inside my biome, get my caves").
    #[test]
    fn abi_query_agrees_with_the_carver() {
        for table in tables() {
            let field = CaveField::with_table(0xB10, table);
            let mut seen = std::collections::BTreeSet::new();
            // Underground-biome regions span a few hundred blocks, so the
            // sample is scattered boxes rather than one — a single small box
            // would sit inside one biome and prove nothing about boundaries.
            for (x0, y0, z0) in [
                (-6, -50, 30),
                (-280, -20, 150),
                (190, 8, -240),
                (-140, 40, 320),
                (260, -44, -70),
            ] {
                let (x1, y1, z1) = (x0 + 15, y0 + 15, z0 + 15);
                let lat = field.build_lattice(x0, y0, z0, x1, y1, z1);
                for y in (y0..=y1).step_by(3) {
                    for z in (z0..=z1).step_by(3) {
                        for x in (x0..=x1).step_by(3) {
                            let via_lattice = field.biome_id_lat(&lat, x, y, z);
                            assert_eq!(
                                field.underground_biome_at(x, y, z),
                                via_lattice,
                                "ABI/carver divergence at ({x},{y},{z})"
                            );
                            seen.insert(via_lattice);
                        }
                    }
                }
            }
            assert!(
                seen.len() > 1 || !has_banded_rows(table),
                "sample should span more than one biome"
            );
        }
    }

    /// The box query is a REJECTION gate, so its only real contract is the one
    /// direction: whatever `underground_biome_at` answers anywhere in the box
    /// must appear in the box's set. A bound that tightens — a lattice cell
    /// missed at the box edge, a clamped depth window, a memo keyed too coarsely
    /// — silently deletes every mod feature that gates on it, and nothing else
    /// would notice. Boxes deliberately straddle the memo grid so the snap-out
    /// is exercised rather than the aligned happy path.
    #[test]
    fn the_box_query_never_omits_a_biome_the_point_query_answers() {
        for table in tables() {
            let field = CaveField::with_table(0xB10, table);
            let mut spanned = std::collections::BTreeSet::new();
            for (x0, y0, z0) in [
                (-6, -50, 30),
                (-281, -19, 151),
                (191, 7, -239),
                (-141, 41, 321),
                (263, -45, -71),
            ] {
                let (x1, y1, z1) = (x0 + 15, y0 + 15, z0 + 15);
                let ids = field.underground_biome_ids_in_box([x0, y0, z0], [x1, y1, z1]);
                for y in y0..=y1 {
                    for z in (z0..=z1).step_by(3) {
                        for x in (x0..=x1).step_by(3) {
                            let id = field.underground_biome_at(x, y, z);
                            spanned.insert(id);
                            assert!(
                                ids.contains(id),
                                "box query omitted biome {id} that owns ({x},{y},{z})"
                            );
                        }
                    }
                }
            }
            assert!(
                spanned.len() > 1 || !has_banded_rows(table),
                "sample should span more than one biome, or it proves nothing"
            );
        }
    }

    /// Hazard 1 with the real noise field in the loop: a row's territory comes
    /// from the band it DECLARES, so installing one more pack must move
    /// neither a biome's cells nor a single carve decision outside the
    /// newcomer's own band — even though the newcomer shifts every later row's
    /// registry id AND arms the caliber path (the biome field becomes a carve
    /// input only in the second table). A selection keyed on id, row index, or
    /// load order fails this everywhere at once.
    #[test]
    fn an_added_pack_row_moves_neither_territory_nor_cave_shape() {
        // Registered as the FIRST pack layer, so it pushes `other:quiet` to a
        // different id in the two tables — id drift is the thing under test.
        const NEWCOMER: &str = r#"{"underground_biomes": [
            {"underground_biome": "newcomer:roomy", "field": [0.02, 0.10],
             "lining": {"block": "petramond:moss_block", "shell": 1.5},
             "caliber": {"tunnel": 1.8, "cheese": 0.15, "blend": [0.01, 2]}}]}"#;
        const OTHER: &str = r#"{"underground_biomes": [
            {"underground_biome": "other:quiet", "field": [-0.30, -0.10]}]}"#;

        let without = underground::test_table(&[OTHER]);
        let with = underground::test_table(&[NEWCOMER, OTHER]);
        assert_ne!(
            without.id("other:quiet"),
            with.id("other:quiet"),
            "the newcomer must actually move a later row's id, or this proves nothing"
        );
        let newcomer = with.name(with.id("newcomer:roomy").unwrap()).unwrap();
        assert!(!without.caliber_varies && with.caliber_varies);

        let (a, b) = (
            CaveField::with_table(0x312, without),
            CaveField::with_table(0x312, with),
        );
        let (mut claimed, mut reshaped, mut preserved) = (0usize, 0usize, 0usize);
        for (x0, y0, z0) in [
            (-6, -46, 30),
            (-280, -20, 150),
            (190, -8, -240),
            (260, 8, -70),
        ] {
            let (x1, y1, z1) = (x0 + 23, y0 + 23, z0 + 23);
            let (la, lb) = (
                a.build_lattice(x0, y0, z0, x1, y1, z1),
                b.build_lattice(x0, y0, z0, x1, y1, z1),
            );
            for y in y0..=y1 {
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        let owner_a = without.name(a.biome_id_lat(&la, x, y, z)).unwrap();
                        let owner_b = with.name(b.biome_id_lat(&lb, x, y, z)).unwrap();
                        // The newcomer's band overlaps no other row's, so it
                        // owns EXACTLY the cells its own band covers. Deriving
                        // the answer from the declared band rather than from
                        // the table is the point: an id- or index-keyed
                        // partition claims cells this side rejects.
                        if (0.02..0.10).contains(&lb.biome(x, y, z)) {
                            assert_eq!(owner_b, newcomer, "unclaimed cell in the new band");
                            claimed += 1;
                            reshaped += (a.cut_lat(&la, x, y, z, 96) != b.cut_lat(&lb, x, y, z, 96))
                                as usize;
                            continue;
                        }
                        assert_eq!(
                            owner_a, owner_b,
                            "the newcomer moved {owner_a} out of ({x},{y},{z})"
                        );
                        assert_eq!(
                            a.cut_lat(&la, x, y, z, 96),
                            b.cut_lat(&lb, x, y, z, 96),
                            "cave shape changed outside the newcomer's band at ({x},{y},{z})"
                        );
                        preserved += 1;
                    }
                }
            }
        }
        // Not density pins — proof the sample actually exercised both sides.
        assert!(
            claimed > 0 && preserved > 0,
            "sample straddles the new band"
        );
        assert!(
            reshaped > 0,
            "the newcomer's caliber must reshape its OWN caves"
        );
    }

    /// Batch lattices are world-anchored, so two different boxes covering the same
    /// voxel interpolate identical values: section seams cannot show.
    #[test]
    fn overlapping_lattices_agree_at_shared_voxels() {
        let field = CaveField::new(0xC0FFEE);
        let a = field.build_lattice(0, 0, 0, 15, 15, 15);
        let b = field.build_lattice(-16, 4, 8, 15, 35, 23);
        for &(x, y, z) in &[(0, 4, 8), (7, 15, 15), (15, 12, 9), (3, 8, 15)] {
            let (mut ca, mut cb) = (Col::new(&a, x, z), Col::new(&b, x, z));
            for k in [lane::A, lane::BRANCH, lane::NA, lane::ROUGH, lane::CHEESE] {
                assert_eq!(ca.get(k, y).to_bits(), cb.get(k, y).to_bits());
            }
            assert_eq!(a.biome(x, y, z).to_bits(), b.biome(x, y, z).to_bits());
        }
    }

    /// Wall lining is a shell AROUND carved air, never a replacement for it: a
    /// voxel the carvers open can never simultaneously be lining, and lining only
    /// appears in a bounded band next to carve decisions (guards against a shell
    /// threshold inversion silently turning whole regions into lining). Re-run
    /// with a `lining.shell` multiplier so the knob a pack can turn is covered
    /// by the same ceiling, not just the engine's own shell widths.
    #[test]
    fn lining_shell_is_disjoint_from_carved_air() {
        for table in tables() {
            let field = CaveField::with_table(0xBEEF, table);
            let (x0, y0, z0) = (32, -32, -16);
            let (x1, y1, z1) = (x0 + 31, y0 + 31, z0 + 31);
            let lat = field.build_lattice(x0, y0, z0, x1, y1, z1);
            let surf_y = 90;
            let (mut open, mut shell, mut solid) = (0usize, 0usize, 0usize);
            for y in y0..=y1 {
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        match field.cut_lat(&lat, x, y, z, surf_y) {
                            CaveCut::Open => open += 1,
                            CaveCut::Shell => shell += 1,
                            CaveCut::Solid => solid += 1,
                        }
                    }
                }
            }
            let total = (open + shell + solid) as f64;
            assert!(open > 0, "test volume should contain cave air");
            assert!(shell > 0, "test volume should contain wall shell");
            assert!(
                (shell as f64) < total * 0.5,
                "shell must be a lining, not a region fill ({shell}/{total})"
            );
        }
    }

    /// A declared FLOOR lining is a GUARANTEE, not a coverage percentage: the
    /// spawn rules a pack hangs off it are only as good as its worst cell, and
    /// a 95% floor is a hole a player finds and the author never does.
    ///
    /// Swept over real carved sections, including the two places the rule has
    /// no memory to fall back on — the top voxel of a section (its air
    /// neighbour lives in the next section up) and the world-floor plane the
    /// carvers refuse to cut, which is the flattest, most walkable floor in the
    /// biome and sits in a section the carve otherwise skips outright.
    #[test]
    fn a_declared_floor_lining_paints_every_cave_floor_in_its_biome() {
        // A wide band, so the sweep finds plenty of the row's own cells — but
        // NARROWER rows still win inside it (marble does), which is why the
        // assertion below is gated on who actually owns the cell rather than
        // on the box.
        const FLOORED: &str = r#"{"underground_biomes": [
            {"underground_biome": "moss:everywhere", "field": [-1.5, 1.5], "y": [-64, -16],
             "lining": {"block": "petramond:moss_block", "shell": 1.4,
                        "faces": {"floor_depth": 2, "ceiling": {"weight": 0.0}}},
             "caliber": {"tunnel": 1.6, "cheese": 0.22, "blend": [0.02, 4]}}]}"#;
        let table = underground::test_table(&[FLOORED]);
        let row = table.id("moss:everywhere").expect("the floored row");
        let moss = crate::block::Block::MossBlock.id();
        let (air, stone) = (Block::Air.id(), Block::Stone.id());
        let surf = vec![40i32; SECTION_SIZE * SECTION_SIZE];
        for seed in [0x312u32, 0x1D001, 0x2BEEF] {
            let field = CaveField::with_table(seed, table);
            let mut floors = 0usize;
            for (cx, cz) in [(0, 0), (-3, 2), (7, -5), (11, 9), (-14, -8), (4, 17)] {
                // Stack of sections over one column, so the voxel above a
                // section's top voxel is a real neighbour and not a guess.
                let carved: Vec<Section> = (-4..=-2)
                    .map(|cy| {
                        let mut s = Section::new(cx, cy, cz);
                        s.blocks_mut().fill(stone);
                        field.carve_section(&mut s, &surf);
                        s
                    })
                    .collect();
                let at = |wy: i32, x: usize, z: usize| {
                    let cy = wy.div_euclid(SECTION_SIZE as i32);
                    carved.get((cy + 4) as usize).map(|s| {
                        s.blocks_iter().collect::<Vec<_>>()
                            [section_idx(x, wy.rem_euclid(SECTION_SIZE as i32) as usize, z)]
                    })
                };
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        for wy in -64..-16 {
                            let (Some(here), Some(above)) = (at(wy, x, z), at(wy + 1, x, z)) else {
                                continue;
                            };
                            let wx = cx * SECTION_SIZE as i32 + x as i32;
                            let wz = cz * SECTION_SIZE as i32 + z as i32;
                            if above != air
                                || here == air
                                || field.underground_biome_at(wx, wy, wz) != row
                            {
                                continue;
                            }
                            floors += 1;
                            if here != moss {
                                panic!(
                                    "cave floor at ({wx},{wy},{wz}) is block {here}, not the                                      declared floor lining (seed {seed:#x})"
                                );
                            }
                        }
                    }
                }
            }
            assert!(
                floors > 200,
                "only {floors} cave floors swept (seed {seed:#x})"
            );
        }
    }

    /// A row lining three ORIENTATIONS with three different blocks. Weight-only
    /// fixtures are what let a ceiling-taken-for-a-wall live: the two rules
    /// then write the same block and differ only in how much of it there is.
    const ORIENTED: &str = r#"{"underground_biomes": [
        {"underground_biome": "faces:everywhere", "field": [-1.5, 1.5], "y": [-64, -16],
         "lining": {"block": "petramond:moss_block", "shell": 1.4,
                    "faces": {"floor_depth": 3,
                              "floor": {"block": "petramond:moss_block"},
                              "wall": {"block": "petramond:marble"},
                              "ceiling": {"block": "petramond:gravel"}}},
         "caliber": {"tunnel": 1.6, "cheese": 0.22, "blend": [0.02, 4]}}]}"#;

    /// A row layering a SUBSURFACE under its floor: moss on top, marble for
    /// the two courses below it.
    const LAYERED: &str = r#"{"underground_biomes": [
        {"underground_biome": "faces:layered", "field": [-1.5, 1.5], "y": [-64, -16],
         "lining": {"block": "petramond:moss_block", "shell": 1.4,
                    "faces": {"floor_depth": 3,
                              "floor": {"block": "petramond:moss_block"},
                              "floor_under": {"block": "petramond:marble"}}},
         "caliber": {"tunnel": 1.6, "cheese": 0.22, "blend": [0.02, 4]}}]}"#;

    /// Exactly ONE surface cell per floor course, with the subsurface under
    /// it — including where the course top sits in the box ABOVE. That case is
    /// the whole hazard: a run beginning mid-course cannot see its own top, so
    /// a painter that assumes depth 0 lays a second surface on every section
    /// boundary, which reads as stripes of moss buried in the rock.
    #[test]
    fn a_layered_floor_course_has_one_surface_wherever_the_batch_splits_it() {
        let table = underground::test_table(&[LAYERED]);
        let row = table.id("faces:layered").expect("the layered row");
        let (moss, under) = (Block::MossBlock.id(), Block::Marble.id());
        let (air, stone) = (Block::Air.id(), Block::Stone.id());
        let surf = vec![40i32; SECTION_SIZE * SECTION_SIZE];
        let (mut courses, mut split) = (0usize, 0usize);
        for seed in [0x312u32, 0x1D001, 0x2BEEF] {
            let field = CaveField::with_table(seed, table);
            for (cx, cz) in [(0, 0), (-3, 2), (7, -5), (11, 9)] {
                let carved: Vec<Section> = (-4..=-2)
                    .map(|cy| {
                        let mut s = Section::new(cx, cy, cz);
                        s.blocks_mut().fill(stone);
                        field.carve_section(&mut s, &surf);
                        s
                    })
                    .collect();
                let at = |wy: i32, x: usize, z: usize| {
                    let cy = wy.div_euclid(SECTION_SIZE as i32);
                    carved.get((cy + 4) as usize).map(|s| {
                        s.blocks_iter().collect::<Vec<_>>()
                            [section_idx(x, wy.rem_euclid(SECTION_SIZE as i32) as usize, z)]
                    })
                };
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let wx = cx * SECTION_SIZE as i32 + x as i32;
                        let wz = cz * SECTION_SIZE as i32 + z as i32;
                        for wy in -63..-17 {
                            // A course top: solid, carved air directly above.
                            let (Some(here), Some(above)) = (at(wy, x, z), at(wy + 1, x, z)) else {
                                continue;
                            };
                            if here == air
                                || above != air
                                || field.underground_biome_at(wx, wy, wz) != row
                            {
                                continue;
                            }
                            courses += 1;
                            split += (wy.rem_euclid(SECTION_SIZE as i32) == 0) as usize;
                            assert_eq!(
                                here, moss,
                                "({wx},{wy},{wz}) tops a course but is not the surface \
                                 (seed {seed:#x})"
                            );
                            for d in 1..=2 {
                                let Some(deep) = at(wy - d, x, z) else { break };
                                if deep == air {
                                    break;
                                }
                                assert_eq!(
                                    deep,
                                    under,
                                    "({wx},{},{wz}) is {d} under the course top and should be \
                                     subsurface, not a second surface (seed {seed:#x})",
                                    wy - d
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(courses > 300, "only {courses} course tops swept");
        // Vacuous unless the sweep actually crossed a course split by a
        // section plane, which is the case the depth argument exists for.
        assert!(split > 0, "no course top swept on a section-floor plane");
    }

    /// Which orientation a cell has, and how far a floor course reaches into
    /// it, are properties of the CAVE — not of the box the carve happened to
    /// be batched into. Both are loop-carried state in the column walk, so
    /// both are only as good as what the walk does at a box boundary, and a
    /// 16-block section has a boundary every sixteen voxels.
    ///
    /// Categorical, not statistical: every rock cell over carved air must be
    /// off the WALL rule, and every stone cell within the declared course of a
    /// cave floor must carry the floor block.
    #[test]
    fn face_orientation_and_course_depth_do_not_depend_on_the_batch() {
        const DEPTH: i32 = 3;
        let table = underground::test_table(&[ORIENTED]);
        let row = table.id("faces:everywhere").expect("the oriented row");
        let moss = Block::MossBlock.id();
        let wall = Block::Marble.id();
        let (air, stone) = (Block::Air.id(), Block::Stone.id());
        let surf = vec![40i32; SECTION_SIZE * SECTION_SIZE];
        let mut boundary = 0usize;
        for seed in [0x312u32, 0x1D001, 0x2BEEF] {
            let field = CaveField::with_table(seed, table);
            let (mut ceilings, mut course) = (0usize, 0usize);
            for (cx, cz) in [(0, 0), (-3, 2), (7, -5), (11, 9), (-14, -8), (4, 17)] {
                let carved: Vec<Section> = (-4..=-2)
                    .map(|cy| {
                        let mut s = Section::new(cx, cy, cz);
                        s.blocks_mut().fill(stone);
                        field.carve_section(&mut s, &surf);
                        s
                    })
                    .collect();
                let at = |wy: i32, x: usize, z: usize| {
                    let cy = wy.div_euclid(SECTION_SIZE as i32);
                    carved.get((cy + 4) as usize).map(|s| {
                        s.blocks_iter().collect::<Vec<_>>()
                            [section_idx(x, wy.rem_euclid(SECTION_SIZE as i32) as usize, z)]
                    })
                };
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let wx = cx * SECTION_SIZE as i32 + x as i32;
                        let wz = cz * SECTION_SIZE as i32 + z as i32;
                        let owned = |wy: i32| field.underground_biome_at(wx, wy, wz) == row;
                        for wy in -64..-16 {
                            let (Some(here), Some(below)) = (at(wy, x, z), at(wy - 1, x, z)) else {
                                continue;
                            };
                            // A cell that is a floor too is repainted by the
                            // floor rule, which outranks both side rules.
                            if here != air
                                && below == air
                                && at(wy + 1, x, z) != Some(air)
                                && owned(wy)
                            {
                                ceilings += 1;
                                boundary += (wy.rem_euclid(SECTION_SIZE as i32) == 0) as usize;
                                assert_ne!(
                                    here, wall,
                                    "({wx},{wy},{wz}) is over carved air but took the WALL \
                                     rule (seed {seed:#x})"
                                );
                            }
                            if here != air || !owned(wy - 1) {
                                continue;
                            }
                            for d in 1..=DEPTH {
                                let Some(deep) = at(wy - d, x, z) else { break };
                                if deep == air {
                                    break;
                                }
                                course += 1;
                                assert_eq!(
                                    deep,
                                    moss,
                                    "({wx},{},{wz}) is {d} under a cave floor but is not the \
                                     declared course (seed {seed:#x})",
                                    wy - d
                                );
                            }
                        }
                    }
                }
            }
            assert!(course > 400, "only {course} course cells (seed {seed:#x})");
            assert!(ceilings > 100, "only {ceilings} ceilings (seed {seed:#x})");
        }
        // Coverage, not a shape pin: the assertions above are vacuous unless
        // the sweep reached a ceiling standing on a section FLOOR, which is
        // the only plane where the walk cannot remember what is under it.
        assert!(boundary > 0, "no ceiling swept on a section-floor plane");
    }

    /// The two batch paths must produce the same blocks. They share the column
    /// walk, which is necessary and not sufficient: the walk's carry is seeded
    /// at the box floor and flushed at the box top, and those are different
    /// voxels for a 256-tall chunk and a 16-tall section.
    #[test]
    fn the_two_batch_carve_paths_line_a_cave_identically() {
        // Banded to positive Y so the whole-column path, which starts at y=0,
        // walks the row at all.
        const BANDED: &str = r#"{"underground_biomes": [
            {"underground_biome": "faces:band", "field": [-1.5, 1.5], "y": [0, 40],
             "lining": {"block": "petramond:moss_block", "shell": 1.4,
                        "faces": {"floor_depth": 3,
                                  "floor": {"block": "petramond:moss_block"},
                                  "wall": {"block": "petramond:marble"},
                                  "ceiling": {"block": "petramond:gravel"}}},
             "caliber": {"tunnel": 1.6, "cheese": 0.22, "blend": [0.02, 4]}}]}"#;
        let table = underground::test_table(&[BANDED]);
        let (air, stone) = (Block::Air.id(), Block::Stone.id());
        let csurf = vec![60i32; CHUNK_SX * CHUNK_SZ];
        let ssurf = vec![60i32; SECTION_SIZE * SECTION_SIZE];
        // Rock that is NOT all stone. Only a stone cell takes a lining, so a
        // fixture of pure stone cannot see a course whose reach depends on
        // block ids the neighbouring batch can read and this one cannot.
        let rock = |wx: i32, wy: i32, wz: i32| {
            if (wx * 31 + wy * 7 + wz * 13).rem_euclid(5) == 0 {
                Block::Tuff.id()
            } else {
                stone
            }
        };
        let mut lined = 0usize;
        for seed in [0x312u32, 0x1D001] {
            let field = CaveField::with_table(seed, table);
            for (cx, cz) in [(0, 0), (5, -3)] {
                let mut chunk = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for y in 0..CHUNK_SY {
                        for x in 0..CHUNK_SX {
                            let (wx, wz) = (
                                cx * CHUNK_SX as i32 + x as i32,
                                cz * CHUNK_SZ as i32 + z as i32,
                            );
                            chunk.blocks_slice_mut()[idx(x, y, z)] = rock(wx, y as i32, wz);
                        }
                    }
                }
                field.carve_chunk(&mut chunk, &csurf);
                for cy in 0..3i32 {
                    let mut s = Section::new(cx, cy, cz);
                    s.edit_ids_bulk(|dst| {
                        for z in 0..SECTION_SIZE {
                            for ly in 0..SECTION_SIZE {
                                for x in 0..SECTION_SIZE {
                                    let (wx, wz) = (
                                        cx * SECTION_SIZE as i32 + x as i32,
                                        cz * SECTION_SIZE as i32 + z as i32,
                                    );
                                    let wy = cy * SECTION_SIZE as i32 + ly as i32;
                                    dst[section_idx(x, ly, z)] = rock(wx, wy, wz);
                                }
                            }
                        }
                    });
                    field.carve_section(&mut s, &ssurf);
                    for z in 0..SECTION_SIZE {
                        for ly in 0..SECTION_SIZE {
                            for x in 0..SECTION_SIZE {
                                let wy = cy as usize * SECTION_SIZE + ly;
                                let want = chunk.blocks_slice()[idx(x, wy, z)];
                                let got = s.block_raw(x, ly, z);
                                assert_eq!(
                                    got, want,
                                    "section and chunk disagree at ({x},{wy},{z}) of chunk \
                                     ({cx},{cz}) seed {seed:#x}"
                                );
                                let base = rock(
                                    cx * SECTION_SIZE as i32 + x as i32,
                                    wy as i32,
                                    cz * SECTION_SIZE as i32 + z as i32,
                                );
                                lined += (got != base && got != air) as usize;
                            }
                        }
                    }
                }
            }
        }
        assert!(lined > 500, "only {lined} lined cells compared");
    }

    /// Cheese caverns must be depth-scaled: the carve threshold at the world floor
    /// is strictly more permissive than near the surface.
    #[test]
    fn cheese_threshold_grows_with_depth() {
        assert!(cheese_threshold(-60) > cheese_threshold(0));
        assert!(cheese_threshold(0) > cheese_threshold(64));
        assert_eq!(cheese_threshold(100), CAVE_CHEESE_T_SHALLOW);
        assert_eq!(cheese_threshold(-64), CAVE_CHEESE_T_DEEP);
    }
}
