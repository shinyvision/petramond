//! Worldgen dressing for the dripstone caves: pointed-dripstone runs off the
//! habitat's ceilings and floors, crowded into formations; inside a
//! formation's core, CLUSTERS (a run with four shorter arms) and CONES (a
//! stepped mound of dripstone blocks tapering into a run), and a cone whose
//! core meets the opposite surface becomes a floor-to-ceiling COLUMN with a
//! mirrored cone at its foot.
//!
//! SEAM CONTRACT, the one `crate::probe` states: sections generate in any
//! order on any thread, and a formation may straddle several of them. Every
//! decision is therefore a pure function of `(seed, root cell)` plus the
//! POSITIONAL terrain — never of the dispatching section. Cells this section
//! owns are read from its snapshot (where disagreement is impossible);
//! everything outside it is read through `TerrainReads`, and a cell that was
//! not probed is UNKNOWN: never rooted on, never grown into. The scan window
//! reaches far enough past the section that any formation with a cell inside
//! it is found from every side.
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

use std::collections::BTreeSet;

use mod_sdk::*;

use super::{Dripstone, BIOME_TOP_Y};
use crate::probe::{self, Pad, TerrainReads};

const SALT_ROOT: u64 = 0x0E58_2000_0000_0001;
const SALT_FORMATION: u64 = 0x0E58_2000_0000_0002;

const MAX_LEN: i32 = 6;
const COLUMN_REACH: i32 = 14;
const CONE_RUN: i32 = 2;
const CONE_WIDE: [i32; 6] = [2, 2, 1, 1, 0, 0];
const CONE_NARROW: [i32; 4] = [1, 1, 0, 0];

const MARGIN_CONE: i32 = COLUMN_REACH + CONE_WIDE.len() as i32 + CONE_RUN;
const SIDE_MARGIN: i32 = 2;
const REACH_PAD: Pad = Pad {
    xz: SIDE_MARGIN,
    down: MARGIN_CONE,
    up: MARGIN_CONE,
};

/// One centre per lattice cell, owns a disc of `FORMATION_R` blocks. Root density per-mille goes
/// from the core value at centre to the rim value at edge. Columns in no formation keep the stray
/// value. Inner half of a formation is its CORE, where clusters and cones roll.
const FORMATION_LATTICE: i32 = 32;
const FORMATION_R: (i32, i32) = (7, 13);
const CORE_PER_MILLE: i32 = 420;
const RIM_PER_MILLE: i32 = 110;
const STRAY_PER_MILLE: i32 = 30;
const FORMATIONS: ColonyField = ColonyField {
    salt: SALT_FORMATION,
    lattice: FORMATION_LATTICE,
    one_in: 1,
    radius: FORMATION_R,
    core: CORE_PER_MILLE,
    rim: RIM_PER_MILLE,
    stray: STRAY_PER_MILLE,
};
const CLUSTER_ONE_IN: i32 = 4;
const CONE_ONE_IN: i32 = 30;
const CONE_WIDE_ONE_IN: i32 = 2;
const COLUMN_ONE_IN: i32 = 2;

pub const GEN_FILTER: GenFeatureFilter =
    GenFeatureFilter::y_band(i32::MIN, BIOME_TOP_Y + MARGIN_CONE);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Kind {
    Single,
    Cluster,
    Cone { wide: bool, column: bool },
}

struct Root {
    p: [i32; 3],
    len: i32,
    kind: Kind,
}

impl Root {
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
    Fluid,
    Solid,
    Unknown,
}

impl Space {
    fn of(answer: Option<TerrainSpace>) -> Space {
        match answer {
            Some(TerrainSpace::Air) => Space::Air,
            Some(TerrainSpace::Fluid) => Space::Fluid,
            Some(TerrainSpace::Solid) => Space::Solid,
            None => Space::Unknown,
        }
    }
}

struct Foot {
    base: [i32; 3],
    up: i32,
    wide: bool,
}

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
    if !probe::in_reach(ours, origin, REACH_PAD, underground_biomes_in_box) {
        return Vec::new();
    }

    let roots = gather(seed, origin);
    if roots.is_empty() {
        return Vec::new();
    }

    let Some(biomes) = probe::ask(roots.iter().map(|r| r.p).collect(), underground_biome_at) else {
        return Vec::new();
    };
    let roots: Vec<Root> = roots
        .into_iter()
        .zip(biomes)
        .filter(|(_, b)| *b == ours)
        .map(|(r, _)| r)
        .collect();
    if roots.is_empty() {
        return Vec::new();
    }

    let mut probes = TerrainReads::new();
    probes.ask_unseen(ctx, roots.iter().flat_map(cells_read));
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

    let mut w = Writes::default();
    let feet: Vec<Foot> = {
        let space = |c: [i32; 3]| snapshot(c).unwrap_or_else(|| Space::of(probes.space(c)));
        roots
            .iter()
            .filter_map(|r| place_root(&space, &mut w, r))
            .collect()
    };

    if !feet.is_empty() {
        probes.ask_unseen(
            ctx,
            feet.iter()
                .flat_map(|f| cone_cells(f.base, f.up, f.wide, false)),
        );
        let space = |c: [i32; 3]| snapshot(c).unwrap_or_else(|| Space::of(probes.space(c)));
        for f in &feet {
            place_cone(&space, &mut w, f.base, f.up, f.wide, true);
        }
    }
    w.into_writes(d, ctx)
}

fn place_root(space: &dyn Fn([i32; 3]) -> Space, w: &mut Writes, r: &Root) -> Option<Foot> {
    let down = orientation(space, r.p)?;
    let step = if down { -1 } else { 1 };
    match r.kind {
        Kind::Single => place_run(space, w, r.p, step, r.len),
        Kind::Cluster => {
            place_run(space, w, r.p, step, r.len);
            for (i, arm) in arms(r.p).into_iter().enumerate() {
                if orientation(space, arm) == Some(down) {
                    let len = (r.len - 1 - (i as i32 & 1)).max(1);
                    place_run(space, w, arm, step, len);
                }
            }
        }
        Kind::Cone { wide, column } => {
            let foot = if column {
                column_foot(space, r.p, step)
            } else {
                None
            };
            place_cone(space, w, r.p, step, wide, foot.is_some());
            let g = foot?;
            for k in 0..g {
                w.solid.insert([r.p[0], r.p[1] + step * k, r.p[2]]);
            }
            return Some(Foot {
                base: [r.p[0], r.p[1] + step * (g - 1), r.p[2]],
                up: -step,
                wide,
            });
        }
    }
    None
}

impl Writes {
    fn into_writes(self, d: &Dripstone, ctx: &GenCtx) -> Vec<GenWrite> {
        let inside = |c: [i32; 3]| ctx.block(c).is_some();
        let mut out: Vec<GenWrite> = Vec::new();
        for c in &self.solid {
            if inside(*c) {
                out.push((*c, d.block));
            }
        }
        for c in self.hanging.difference(&self.solid) {
            if inside(*c) {
                out.push((*c, d.stalactite));
            }
        }
        for c in self.standing.difference(&self.solid) {
            if inside(*c) && !self.hanging.contains(c) {
                out.push((*c, d.stalagmite));
            }
        }
        out
    }
}

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

fn arms(p: [i32; 3]) -> [[i32; 3]; 4] {
    [
        [p[0] + 1, p[1], p[2]],
        [p[0] - 1, p[1], p[2]],
        [p[0], p[1], p[2] + 1],
        [p[0], p[1], p[2] - 1],
    ]
}

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

fn formation_at(seed: u32, wx: i32, wz: i32) -> (i32, bool) {
    let (density, owner) = FORMATIONS.densest(seed, wx, wz, |_| ());
    (density, owner.is_some_and(|f| 2 * f.distance <= f.radius))
}

#[cfg(test)]
mod tests {
    use super::*;

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
