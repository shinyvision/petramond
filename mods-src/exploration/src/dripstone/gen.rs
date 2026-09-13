//! Worldgen dressing for the dripstone caves: pointed-dripstone runs off the
//! habitat's ceilings and floors, crowded into formations; inside a
//! formation's core, CLUSTERS (a run with four shorter arms) and CONES (a
//! stepped mound of dripstone blocks tapering into a run), and a cone whose
//! core meets the opposite surface becomes a floor-to-ceiling COLUMN with a
//! mirrored cone at its foot.
//!
//! SEAM CONTRACT, the same one `cavern.rs` states: sections generate in any
//! order on any thread, and a formation may straddle several of them. Every
//! decision is therefore a pure function of `(seed, root cell)` plus the
//! POSITIONAL terrain — never of the dispatching section. Cells this section
//! owns are read from its snapshot (where disagreement is impossible);
//! everything outside it is read through `terrain_space_at`, and a cell that
//! was not probed is UNKNOWN: never rooted on, never grown into. The scan
//! window reaches far enough past the section that any formation with a cell
//! inside it is found from every side.
//!
//! Formations never block one another. Cells are resolved by a fixed
//! PRECEDENCE (block over hanging over standing), so a section that sees only
//! part of an overlap still writes exactly the cells the rest would concede —
//! the cell-wise union is section-order independent, where a first-come claim
//! would not be.
//!
//! HOST-CALL BUDGET: one box gate, one biome batch over the rolled roots, one
//! terrain batch over every cell outside the section those roots may read,
//! and a second terrain batch only when a column's foot needs its discs.

use std::collections::{BTreeMap, BTreeSet};

use mod_sdk::*;

use super::{Dripstone, BIOME_TOP_Y};
use crate::cavern::batched;

/// Frozen positional-RNG salts (append-only in practice).
const SALT_ROOT: u64 = 0x0E58_2000_0000_0001;
const SALT_FORMATION: u64 = 0x0E58_2000_0000_0002;

/// Longest plain run worldgen places. Growth may lengthen it later.
const MAX_LEN: i32 = 6;
/// Farthest a cone's core reaches for the opposite surface before it stays
/// a cone.
const COLUMN_REACH: i32 = 14;
/// The run a cone ends in when it stays one.
const CONE_RUN: i32 = 2;
/// A cone's disc radii per layer from its root, by rolled width.
const CONE_WIDE: [i32; 6] = [2, 2, 1, 1, 0, 0];
const CONE_NARROW: [i32; 4] = [1, 1, 0, 0];

/// Rows scanned past the section top and bottom, and columns past each
/// side: how far a cone (with its column and mirrored foot) can reach in.
/// Plain runs and clusters reach far less and are filtered per candidate.
const MARGIN_CONE: i32 = COLUMN_REACH + CONE_WIDE.len() as i32 + CONE_RUN;
const SIDE_MARGIN: i32 = 2;

/// Formations: one centre per lattice cell, each owning a disc of
/// `FORMATION_R` blocks. Per-mille root density runs from the core value at
/// the centre to the rim value at the edge; columns in no formation keep
/// the stray value. The inner half of a formation is its CORE, where
/// clusters and cones roll.
const FORMATION_LATTICE: i32 = 32;
const FORMATION_R: (i32, i32) = (7, 13);
const CORE_PER_MILLE: i32 = 420;
const RIM_PER_MILLE: i32 = 110;
const STRAY_PER_MILLE: i32 = 30;
const CLUSTER_ONE_IN: i32 = 4;
const CONE_ONE_IN: i32 = 30;
/// Of the cones, how many are wide, and how many try for a column.
const CONE_WIDE_ONE_IN: i32 = 2;
const COLUMN_ONE_IN: i32 = 2;

/// Everything from the world floor to the habitat's top plus the margin a
/// root above it reaches.
pub const GEN_FILTER: GenFeatureFilter =
    GenFeatureFilter::y_band(i32::MIN, BIOME_TOP_Y + MARGIN_CONE);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Kind {
    Single,
    /// The run plus four shorter arms off the same surface.
    Cluster,
    /// A stepped mound of dripstone blocks ending in a run; `column` = it
    /// reaches for the opposite surface first.
    Cone {
        wide: bool,
        column: bool,
    },
}

/// A rolled root cell: a formation hangs or stands from here, once the
/// terrain says which (or neither).
struct Root {
    p: [i32; 3],
    len: i32,
    kind: Kind,
}

impl Root {
    /// Farthest cell this root can write from `p`: along the axis, and to
    /// the side.
    fn reach(&self) -> (i32, i32) {
        match self.kind {
            Kind::Single => (self.len, 0),
            Kind::Cluster => (self.len, 1),
            Kind::Cone { wide, .. } => (MARGIN_CONE, if wide { 2 } else { 1 }),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Space {
    Air,
    /// Any fluid: a formation neither roots in nor grows through one.
    Fluid,
    Solid,
    /// Outside the section and not among the probed cells.
    Unknown,
}

/// A column's foot, resolved in the first pass, whose mirrored cone needs
/// its own discs probed.
struct Foot {
    /// The first cell of the foot's cone, just off the opposite surface.
    base: [i32; 3],
    /// Toward the column's root: which way the foot's cone tapers.
    up: i32,
    wide: bool,
}

/// What the dressing writes, by precedence: a block beats a hanging
/// segment, which beats a standing one.
#[derive(Default)]
struct Writes {
    solid: BTreeSet<[i32; 3]>,
    hanging: BTreeSet<[i32; 3]>,
    standing: BTreeSet<[i32; 3]>,
}

pub fn generate(d: &Dripstone, ctx: &GenCtx) -> Vec<GenWrite> {
    if !GEN_FILTER.intersects(ctx.section_pos()[1], &[]) {
        return Vec::new();
    }
    let Some(ours) = d.biome else {
        return Vec::new();
    };
    let origin = ctx.origin_world();
    let seed = ctx.seed();
    let in_reach = underground_biomes_in_box(
        [
            origin[0] - SIDE_MARGIN,
            origin[1] - MARGIN_CONE,
            origin[2] - SIDE_MARGIN,
        ],
        [
            origin[0] + 15 + SIDE_MARGIN,
            origin[1] + 15 + MARGIN_CONE,
            origin[2] + 15 + SIDE_MARGIN,
        ],
    )
    .contains(&ours);
    if !in_reach {
        return Vec::new();
    }

    let roots = gather(seed, origin);
    if roots.is_empty() {
        return Vec::new();
    }

    // --- ONE biome batch: a root grows only in the habitat ------------
    let want = roots.len();
    let biomes = batched(roots.iter().map(|r| r.p).collect(), underground_biome_at);
    if biomes.len() != want {
        return Vec::new();
    }
    let roots: Vec<Root> = roots
        .into_iter()
        .zip(biomes)
        .filter(|(_, b)| *b == ours)
        .map(|(r, _)| r)
        .collect();
    if roots.is_empty() {
        return Vec::new();
    }

    // --- ONE terrain batch: every cell a root may read outside this
    // section ----------------------------------------------------------------
    let inside = |c: [i32; 3]| ctx.block(c).is_some();
    let mut probes = Probes::default();
    probes.ask(roots.iter().flat_map(cells_read).filter(|c| !inside(*c)));
    let snapshot = |c: [i32; 3]| -> Option<Space> {
        ctx.block(c).map(|b| {
            if b == d.air {
                Space::Air
            } else if d.fluids.contains(b) {
                Space::Fluid
            } else {
                Space::Solid
            }
        })
    };

    // --- resolve, first pass ---------------------------------------------
    let mut w = Writes::default();
    let mut feet: Vec<Foot> = Vec::new();
    {
        let space = |c: [i32; 3]| snapshot(c).unwrap_or_else(|| probes.space(c));
        for r in &roots {
            let Some(down) = orientation(&space, r.p) else {
                continue;
            };
            let step = if down { -1 } else { 1 };
            match r.kind {
                Kind::Single => place_run(&space, &mut w, r.p, step, r.len),
                Kind::Cluster => {
                    place_run(&space, &mut w, r.p, step, r.len);
                    for (i, arm) in arms(r.p).into_iter().enumerate() {
                        // Shorter than the centre by one or two, alternating
                        // by side so a cluster reads as a ragged crown.
                        if orientation(&space, arm) == Some(down) {
                            let len = (r.len - 1 - (i as i32 & 1)).max(1);
                            place_run(&space, &mut w, arm, step, len);
                        }
                    }
                }
                Kind::Cone { wide, column } => {
                    let foot = if column {
                        column_foot(&space, r.p, step)
                    } else {
                        None
                    };
                    place_cone(&space, &mut w, r.p, step, wide, foot.is_some());
                    if let Some(g) = foot {
                        for k in 0..g {
                            w.solid.insert([r.p[0], r.p[1] + step * k, r.p[2]]);
                        }
                        feet.push(Foot {
                            base: [r.p[0], r.p[1] + step * (g - 1), r.p[2]],
                            up: -step,
                            wide,
                        });
                    }
                }
            }
        }
    }

    // --- second pass: a column's foot, once its floor is known -----------
    if !feet.is_empty() {
        probes.ask(
            feet.iter()
                .flat_map(|f| cone_cells(f.base, f.up, f.wide, false))
                .filter(|c| !inside(*c)),
        );
        let space = |c: [i32; 3]| snapshot(c).unwrap_or_else(|| probes.space(c));
        for f in &feet {
            place_cone(&space, &mut w, f.base, f.up, f.wide, true);
        }
    }

    let mut out: Vec<GenWrite> = Vec::new();
    for c in &w.solid {
        if inside(*c) {
            out.push((*c, d.block));
        }
    }
    for c in w.hanging.difference(&w.solid) {
        if inside(*c) {
            out.push((*c, d.stalactite));
        }
    }
    for c in w.standing.difference(&w.solid) {
        if inside(*c) && !w.hanging.contains(c) {
            out.push((*c, d.stalagmite));
        }
    }
    out
}

/// The probed cells outside the section and their answers. A later `ask`
/// appends; earlier answers keep their indices, and the index is built FROM
/// the request order so an answer can never be read back against a
/// different cell.
#[derive(Default)]
struct Probes {
    index: BTreeMap<[i32; 3], usize>,
    spaces: Vec<TerrainSpace>,
    failed: bool,
}

impl Probes {
    fn ask(&mut self, cells: impl Iterator<Item = [i32; 3]>) {
        let fresh: Vec<[i32; 3]> = cells
            .filter(|c| !self.index.contains_key(c))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if fresh.is_empty() {
            return;
        }
        let base = self.spaces.len();
        for (i, c) in fresh.iter().enumerate() {
            self.index.insert(*c, base + i);
        }
        let want = fresh.len();
        let reply = batched(fresh, terrain_space_at);
        if reply.len() != want {
            // A refused batch leaves every cell unknown.
            self.failed = true;
            return;
        }
        self.spaces.extend(reply);
    }

    fn space(&self, c: [i32; 3]) -> Space {
        if self.failed {
            return Space::Unknown;
        }
        match self.index.get(&c).map(|&i| self.spaces[i]) {
            Some(TerrainSpace::Air) => Space::Air,
            Some(TerrainSpace::Fluid) => Space::Fluid,
            Some(TerrainSpace::Solid) => Space::Solid,
            None => Space::Unknown,
        }
    }
}

/// Whether `root` hangs (`Some(true)`: open with solid above), stands
/// (`Some(false)`: open with solid below), or is no root at all. A one-tall
/// gap hangs — the same choice placement makes for a ceiling click.
fn orientation(space: &dyn Fn([i32; 3]) -> Space, root: [i32; 3]) -> Option<bool> {
    if space(root) != Space::Air {
        return None;
    }
    if space([root[0], root[1] + 1, root[2]]) == Space::Solid {
        return Some(true);
    }
    if space([root[0], root[1] - 1, root[2]]) == Space::Solid {
        return Some(false);
    }
    None
}

/// A run of `len` from `root` along `step`, stopping at the first cell that
/// is not open air.
fn place_run(
    space: &dyn Fn([i32; 3]) -> Space,
    w: &mut Writes,
    root: [i32; 3],
    step: i32,
    len: i32,
) {
    let set = if step < 0 {
        &mut w.hanging
    } else {
        &mut w.standing
    };
    for k in 0..len {
        let c = [root[0], root[1] + step * k, root[2]];
        if space(c) != Space::Air {
            break;
        }
        set.insert(c);
    }
}

fn cone_profile(wide: bool) -> &'static [i32] {
    if wide {
        &CONE_WIDE
    } else {
        &CONE_NARROW
    }
}

/// Every cell of a cone rooted at `root` tapering along `step`: its disc
/// layers, then (unless it is a column's foot, whose run is the shaft) the
/// run it ends in.
fn cone_cells(root: [i32; 3], step: i32, wide: bool, with_run: bool) -> Vec<[i32; 3]> {
    let profile = cone_profile(wide);
    let mut v = Vec::new();
    for (k, &r) in profile.iter().enumerate() {
        let y = root[1] + step * k as i32;
        for dx in -r..=r {
            for dz in -r..=r {
                if dx * dx + dz * dz <= r * r {
                    v.push([root[0] + dx, y, root[2] + dz]);
                }
            }
        }
    }
    if with_run {
        for k in 0..CONE_RUN {
            v.push([
                root[0],
                root[1] + step * (profile.len() as i32 + k),
                root[2],
            ]);
        }
    }
    v
}

/// Write a cone: dripstone blocks layer by layer while the core stays open
/// (a disc cell the rock already holds is skipped), then the run its tip
/// becomes — unless it is a column, whose core is the shaft.
fn place_cone(
    space: &dyn Fn([i32; 3]) -> Space,
    w: &mut Writes,
    root: [i32; 3],
    step: i32,
    wide: bool,
    is_column: bool,
) {
    let profile = cone_profile(wide);
    let mut layers = 0;
    for (k, &r) in profile.iter().enumerate() {
        let y = root[1] + step * k as i32;
        if space([root[0], y, root[2]]) != Space::Air {
            break;
        }
        layers = k + 1;
        for dx in -r..=r {
            for dz in -r..=r {
                let c = [root[0] + dx, y, root[2] + dz];
                if dx * dx + dz * dz <= r * r && space(c) == Space::Air {
                    w.solid.insert(c);
                }
            }
        }
    }
    if layers == profile.len() && !is_column {
        let tip = [root[0], root[1] + step * layers as i32, root[2]];
        place_run(space, w, tip, step, CONE_RUN);
    }
}

/// The distance from `root` along `step` to the opposite surface, when a
/// column can be built: every core cell up to it open, the surface itself
/// solid, and room for both cones with a cell of shaft between (a column
/// shorter than that is two cones touching, which a plain cone already is).
fn column_foot(space: &dyn Fn([i32; 3]) -> Space, root: [i32; 3], step: i32) -> Option<i32> {
    let room = 2 * CONE_NARROW.len() as i32 + 1;
    for g in 1..=COLUMN_REACH {
        match space([root[0], root[1] + step * g, root[2]]) {
            Space::Air => continue,
            Space::Solid => return (g >= room).then_some(g),
            _ => return None,
        }
    }
    None
}

/// The four side neighbours a cluster's arms root in.
fn arms(p: [i32; 3]) -> [[i32; 3]; 4] {
    [
        [p[0] + 1, p[1], p[2]],
        [p[0] - 1, p[1], p[2]],
        [p[0], p[1], p[2] + 1],
        [p[0], p[1], p[2] - 1],
    ]
}

/// Every cell resolving `r` may read in its first pass: the root's own
/// cell and both vertical neighbours (the orientation), and its whole
/// footprint in BOTH directions, since which one applies is not known until
/// the terrain answers.
fn cells_read(r: &Root) -> Vec<[i32; 3]> {
    let mut v = Vec::new();
    let mut column = |root: [i32; 3], len: i32| {
        for k in -len..=len {
            v.push([root[0], root[1] + k, root[2]]);
        }
    };
    match r.kind {
        Kind::Single => column(r.p, r.len),
        Kind::Cluster => {
            column(r.p, r.len);
            for arm in arms(r.p) {
                // An arm's run is at most `len - 1` long, and its orientation
                // reads one cell either way of the root — never less.
                column(arm, (r.len - 1).max(1));
            }
        }
        Kind::Cone {
            wide,
            column: is_column,
        } => {
            column(r.p, if is_column { COLUMN_REACH } else { 1 });
            for step in [-1, 1] {
                v.extend(cone_cells(r.p, step, wide, true));
            }
        }
    }
    v
}

/// Every rolled root whose formation can reach the section at `origin`.
/// Pure: no host calls, so the roll filters the candidates before anything
/// crosses the ABI. The window is the section plus the largest margin any
/// kind reaches; a margin candidate is kept only when ITS kind's reach puts
/// a cell inside the section.
fn gather(seed: u32, origin: [i32; 3]) -> Vec<Root> {
    let mut roots = Vec::new();
    for lz in -SIDE_MARGIN..16 + SIDE_MARGIN {
        for lx in -SIDE_MARGIN..16 + SIDE_MARGIN {
            let (x, z) = (origin[0] + lx, origin[2] + lz);
            let side_away = (-lx).max(lx - 15).max(-lz).max(lz - 15).max(0);
            let (density, core) = formation_at(seed, x, z);
            for ly in -MARGIN_CONE..16 + MARGIN_CONE {
                let y = origin[1] + ly;
                let mut rng = GenRng::positional(seed, SALT_ROOT, x, y, z);
                if rng.next_i32(0, 999) >= density {
                    continue;
                }
                let len = run_len(&mut rng);
                let kind = roll_kind(&mut rng, core);
                let root = Root {
                    p: [x, y, z],
                    len,
                    kind,
                };
                let (axial, side) = root.reach();
                let rows_away = (-ly).max(ly - 15).max(0);
                if side_away > side || axial <= rows_away {
                    continue;
                }
                roots.push(root);
            }
        }
    }
    roots
}

/// Which formation a root grows: only a core rolls clusters and cones.
fn roll_kind(rng: &mut GenRng, core: bool) -> Kind {
    if !core {
        return Kind::Single;
    }
    if rng.next_i32(0, CONE_ONE_IN - 1) == 0 {
        return Kind::Cone {
            wide: rng.next_i32(0, CONE_WIDE_ONE_IN - 1) == 0,
            column: rng.next_i32(0, COLUMN_ONE_IN - 1) == 0,
        };
    }
    if rng.next_i32(0, CLUSTER_ONE_IN - 1) == 0 {
        Kind::Cluster
    } else {
        Kind::Single
    }
}

/// Run length off the root's own stream: mostly short, occasionally long.
fn run_len(rng: &mut GenRng) -> i32 {
    match rng.next_i32(0, 999) {
        r if r < 380 => 1,
        r if r < 660 => 2,
        r if r < 830 => 3,
        r if r < 930 => 4,
        r if r < 975 => 5,
        _ => MAX_LEN,
    }
}

/// The root density (per mille) at a column and whether it lies in a
/// formation's inner half — the `patch_at` colony rule with a linear
/// falloff, so spikes crowd into formations with sparse strays between.
fn formation_at(seed: u32, wx: i32, wz: i32) -> (i32, bool) {
    let (mut best, mut core) = (STRAY_PER_MILLE, false);
    let cell = |v: i32| v.div_euclid(FORMATION_LATTICE);
    for lz in cell(wz - FORMATION_R.1)..=cell(wz + FORMATION_R.1) {
        for lx in cell(wx - FORMATION_R.1)..=cell(wx + FORMATION_R.1) {
            let mut rng = GenRng::positional(seed, SALT_FORMATION, lx, 0, lz);
            let cx = lx * FORMATION_LATTICE + rng.next_i32(0, FORMATION_LATTICE - 1);
            let cz = lz * FORMATION_LATTICE + rng.next_i32(0, FORMATION_LATTICE - 1);
            let r = rng.next_i32(FORMATION_R.0, FORMATION_R.1);
            let (dx, dz) = (wx - cx, wz - cz);
            let d2 = dx * dx + dz * dz;
            if d2 > r * r {
                continue;
            }
            let d = crate::cavern::isqrt(d2);
            let dens = CORE_PER_MILLE + (RIM_PER_MILLE - CORE_PER_MILLE) * d / r.max(1);
            if dens > best {
                best = dens;
                core = 2 * d <= r;
            }
        }
    }
    (best, core)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root's formation must be derived by every section it reaches: the
    /// gather window has to admit a candidate exactly as far away as its
    /// kind reaches, and no further.
    #[test]
    fn the_margin_admits_exactly_the_roots_whose_reach_touches_the_section() {
        let seed = 7;
        let (mut any, mut plain_far) = (0, 0);
        for sx in 0..40 {
            let origin = [sx * 16, 32, 0];
            for r in gather(seed, origin) {
                any += 1;
                let (axial, side) = r.reach();
                let rows_away = (origin[1] - r.p[1]).max(r.p[1] - (origin[1] + 15)).max(0);
                let side_away = (origin[0] - r.p[0])
                    .max(r.p[0] - (origin[0] + 15))
                    .max(origin[2] - r.p[2])
                    .max(r.p[2] - (origin[2] + 15))
                    .max(0);
                assert!(axial > rows_away, "{:?} rows away with reach {axial}", r.p);
                assert!(
                    side >= side_away,
                    "{:?} columns away, side reach {side}",
                    r.p
                );
                if r.kind == Kind::Single && rows_away > 0 {
                    plain_far += 1;
                }
            }
        }
        assert!(
            any > 0 && plain_far > 0,
            "the band yields reaching roots at all"
        );
    }

    /// Formations are what make the caves read as caves rather than a
    /// uniform sprinkle: the density must be strongly bimodal over an area.
    #[test]
    fn formations_concentrate_the_roots() {
        let seed = 11;
        let (mut stray, mut dense) = (0, 0);
        for z in 0..256 {
            for x in 0..256 {
                let (d, _) = formation_at(seed, x, z);
                if d == STRAY_PER_MILLE {
                    stray += 1;
                } else if d >= CORE_PER_MILLE / 2 {
                    dense += 1;
                }
            }
        }
        assert!(stray > 256 * 256 / 3, "most columns are strays: {stray}");
        assert!(dense > 0, "some columns sit in a core");
    }

    /// A one-tall gap is a ceiling, a cell with rock on neither side is no
    /// root, and an unknown cell is never a surface.
    #[test]
    fn orientation_prefers_hanging_and_refuses_open_air() {
        let solid_above = |c: [i32; 3]| if c[1] >= 1 { Space::Solid } else { Space::Air };
        assert_eq!(orientation(&solid_above, [0, 0, 0]), Some(true));
        let solid_below = |c: [i32; 3]| if c[1] <= -1 { Space::Solid } else { Space::Air };
        assert_eq!(orientation(&solid_below, [0, 0, 0]), Some(false));
        let sandwiched = |c: [i32; 3]| if c[1] == 0 { Space::Air } else { Space::Solid };
        assert_eq!(orientation(&sandwiched, [0, 0, 0]), Some(true));
        let unknown_above = |c: [i32; 3]| {
            if c[1] >= 1 {
                Space::Unknown
            } else {
                Space::Air
            }
        };
        assert_eq!(orientation(&unknown_above, [0, 0, 0]), None);
        assert_eq!(orientation(&|_| Space::Air, [0, 0, 0]), None);
        assert_eq!(orientation(&|_| Space::Fluid, [0, 0, 0]), None);
    }

    /// A column is two cones joined by a shaft; where the gap is too short
    /// for both it stays a cone, and a cone clipped by rock keeps only the
    /// open cells.
    #[test]
    fn a_column_needs_room_for_both_cones_and_a_cone_clips_to_open_air() {
        let cave = |c: [i32; 3]| {
            if c[1] >= 10 || c[1] <= 0 {
                Space::Solid
            } else {
                Space::Air
            }
        };
        assert_eq!(column_foot(&cave, [0, 9, 0], -1), Some(9));
        let low = |c: [i32; 3]| {
            if c[1] >= 10 || c[1] <= 4 {
                Space::Solid
            } else {
                Space::Air
            }
        };
        assert_eq!(
            column_foot(&low, [0, 9, 0], -1),
            None,
            "too short for two cones"
        );
        let mut w = Writes::default();
        // A wall at x >= 1 clips the wide cone's east side.
        let walled = |c: [i32; 3]| {
            if c[0] >= 1 || c[1] >= 10 {
                Space::Solid
            } else {
                Space::Air
            }
        };
        place_cone(&walled, &mut w, [0, 9, 0], -1, true, false);
        assert!(w.solid.contains(&[0, 9, 0]) && w.solid.contains(&[-2, 9, 0]));
        assert!(
            !w.solid.iter().any(|c| c[0] >= 1),
            "nothing written into the wall"
        );
        assert!(
            w.hanging.contains(&[0, 3, 0]),
            "the cone ends in a hanging run"
        );
    }

    /// Every cell a cone can write, in either direction, is a cell its
    /// first-pass probe list asked about, and none lies past its declared
    /// reach — the reach, the reads and the writes agree.
    #[test]
    fn a_cones_probe_list_covers_everything_it_can_write() {
        for wide in [false, true] {
            let r = Root {
                p: [3, 40, -7],
                len: 1,
                kind: Kind::Cone { wide, column: true },
            };
            let read: BTreeSet<[i32; 3]> = cells_read(&r).into_iter().collect();
            for step in [-1, 1] {
                let mut w = Writes::default();
                place_cone(&|_| Space::Air, &mut w, r.p, step, wide, false);
                for c in w.solid.iter().chain(&w.hanging).chain(&w.standing) {
                    assert!(read.contains(c), "{c:?} written but never read");
                }
            }
            let (axial, side) = r.reach();
            for c in &read {
                assert!((c[1] - r.p[1]).abs() <= axial && (c[0] - r.p[0]).abs() <= side);
            }
        }
    }
}
