//! Tree features.
//!
//! `TreeFeature` is the generic composition — one `TrunkPlacer` + one
//! `FoliagePlacer` + materials + params — that expresses simple trees as pure
//! data. `BlockyOakFeature` is the stylized oak: a thick trunk on a stepped
//! buttress base, right-angle limbs, and a canopy of overlapping cuboid leaf
//! pads. Its base, limbs and pads are spatially entangled, so it is its own
//! `Feature` rather than a clean trunk/foliage split — mirroring how real
//! engines model complex trees. It still reuses the shared `shapes`
//! primitives and the same `FeatureCtx` predicates.

use crate::block::Block;
use crate::mathh::IVec3;

use super::placers::foliage::FoliagePlacer;
use super::placers::shapes;
use super::placers::trunk::{sample_height, TrunkPlacer};
use super::{Feature, FeatureCtx};
use crate::worldgen::rng::FeatureRng;

/// Generic single-trunk tree: trunk places + returns attach points, then
/// foliage decorates them.
pub struct TreeFeature {
    pub trunk: &'static dyn TrunkPlacer,
    pub foliage: &'static dyn FoliagePlacer,
    pub log: Block,
    pub leaf: Block,
    pub height: (i32, i32),
}

impl Feature for TreeFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        let attach = self.trunk.place(ctx, origin, self.height, self.log, rng);
        self.foliage.place(ctx, &attach, self.leaf, rng);
    }
}

/// Draw a straight 3-D line of branch logs from `a` to `b` (== fancy-oak limbs).
/// Uses `set_branch` so a limb may pass through already-placed leaves.
fn log_line(ctx: &mut FeatureCtx, a: IVec3, b: IVec3, log: Block) {
    let n = (b.x - a.x)
        .abs()
        .max((b.y - a.y).abs())
        .max((b.z - a.z).abs())
        .max(1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = (a.x as f32 + (b.x - a.x) as f32 * t).round() as i32;
        let y = (a.y as f32 + (b.y - a.y) as f32 * t).round() as i32;
        let z = (a.z as f32 + (b.z - a.z) as f32 * t).round() as i32;
        ctx.set_branch(IVec3::new(x, y, z), log);
    }
}

/// The four grid directions branches and roots walk in, in fixed order.
const CARDINALS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

/// Rotate a cardinal direction a quarter turn.
fn rotate(d: (i32, i32), clockwise: bool) -> (i32, i32) {
    if clockwise {
        (d.1, -d.0)
    } else {
        (-d.1, d.0)
    }
}

/// Fisher-Yates shuffle on the tree's own RNG stream.
fn shuffle<T>(items: &mut [T], rng: &mut FeatureRng) {
    for i in (1..items.len()).rev() {
        let j = rng.next_i32(0, i as i32) as usize;
        items.swap(i, j);
    }
}

/// Branch tips — and so leaf-clump centres — are fenced inside this Chebyshev
/// radius of the feature origin: a clump overhangs its centre by at most 5
/// (part offset 2 + part half-extent 3), so `TIP_FENCE + 5 == proto::MARGIN`
/// keeps every leaf inside the seam-consistency margin.
pub(crate) const TIP_FENCE: i32 = crate::worldgen::proto::MARGIN - 5;

/// Trunk cross-section radius by level: a wide 5×5 base flare, a 3×3 shaft,
/// and a bare 1×1 top that the crown clump wraps.
fn trunk_radius(level: i32, height: i32) -> i32 {
    if level < (height / 7).max(3) {
        2
    } else if level < height - 3 {
        1
    } else {
        0
    }
}

/// 2-D grid walk from `(x0, z0)` to `(x1, z1)`: unit steps on one axis at a
/// time, interleaved by fractional progress, so a diagonal becomes an even
/// right-angle zig-zag. Returns every visited cell including the start.
fn grid_line_2d(x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<(i32, i32)> {
    let (mut x, mut z) = (x0, z0);
    let mut points = vec![(x, z)];
    let count_x = (x1 - x0).abs();
    let count_z = (z1 - z0).abs();
    let step_x = if x1 > x0 { 1 } else { -1 };
    let step_z = if z1 > z0 { 1 } else { -1 };
    let mut moves: Vec<(f32, bool)> = Vec::with_capacity((count_x + count_z) as usize);
    for i in 0..count_x {
        moves.push(((i as f32 + 0.5) / count_x as f32, true));
    }
    for i in 0..count_z {
        moves.push(((i as f32 + 0.5) / count_z as f32, false));
    }
    // Stable sort: on equal progress the x move goes first, deterministically.
    moves.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for &(_, is_x) in &moves {
        if is_x {
            x += step_x;
        } else {
            z += step_z;
        }
        points.push((x, z));
    }
    points
}

/// Cuboid leaf box with eroded corners: outer-shell cells on three boundary
/// axes (corners) are dropped 72% of the time, on two (edges) 28% — the cube
/// keeps its blocky read without being a perfect box. Decisions are drawn
/// into a local mask first (never reading world content), then any kept
/// corner whose three inward face-neighbours were all dropped is dropped too,
/// so erosion never leaves a leaf only diagonally attached (it would decay).
/// Over Air/Water only.
fn leaf_box_eroded(
    ctx: &mut FeatureCtx,
    centre: IVec3,
    half: IVec3,
    leaf: Block,
    rng: &mut FeatureRng,
) {
    let (hx, hy, hz) = (half.x.max(1), half.y.max(1), half.z.max(1));
    debug_assert!(hx <= 3 && hy <= 3 && hz <= 3);
    let (sy, sz) = ((2 * hy + 1) as usize, (2 * hz + 1) as usize);
    let idx = |dx: i32, dy: i32, dz: i32| {
        ((dx + hx) as usize * sy + (dy + hy) as usize) * sz + (dz + hz) as usize
    };
    // Stack scratch: a replayed tree erodes ~80 boxes and every chunk within
    // MARGIN replays the tree, so per-box heap allocation is measurable.
    let mut keep = [false; 7 * 7 * 7];
    for dx in -hx..=hx {
        for dy in -hy..=hy {
            for dz in -hz..=hz {
                let exposed =
                    (dx.abs() == hx) as i32 + (dy.abs() == hy) as i32 + (dz.abs() == hz) as i32;
                if exposed == 3 && rng.chance(0.72) {
                    continue;
                }
                if exposed == 2 && rng.chance(0.28) {
                    continue;
                }
                keep[idx(dx, dy, dz)] = true;
            }
        }
    }
    for dx in [-hx, hx] {
        for dy in [-hy, hy] {
            for dz in [-hz, hz] {
                if keep[idx(dx, dy, dz)]
                    && !keep[idx(dx - dx.signum(), dy, dz)]
                    && !keep[idx(dx, dy - dy.signum(), dz)]
                    && !keep[idx(dx, dy, dz - dz.signum())]
                {
                    keep[idx(dx, dy, dz)] = false;
                }
            }
        }
    }
    for dx in -hx..=hx {
        for dy in -hy..=hy {
            for dz in -hz..=hz {
                if keep[idx(dx, dy, dz)] {
                    ctx.set_leaf(
                        IVec3::new(centre.x + dx, centre.y + dy, centre.z + dz),
                        leaf,
                    );
                }
            }
        }
    }
}

/// Hidden support wood for one clump box: a log at the centre, plus four log
/// "arms" two cells out on any axis whose half-extent reaches 3 — every leaf
/// in a `(3, 2, 3)`-half box then reaches a log within the decay flood's
/// distance (6), erosion included.
fn clump_support(ctx: &mut FeatureCtx, centre: IVec3, half: IVec3, log: Block) {
    ctx.set_branch(centre, log);
    if half.x >= 3 {
        ctx.set_branch(IVec3::new(centre.x - 2, centre.y, centre.z), log);
        ctx.set_branch(IVec3::new(centre.x + 2, centre.y, centre.z), log);
    }
    if half.z >= 3 {
        ctx.set_branch(IVec3::new(centre.x, centre.y, centre.z - 2), log);
        ctx.set_branch(IVec3::new(centre.x, centre.y, centre.z + 2), log);
    }
}

/// A clump of 2–4 overlapping eroded leaf boxes around `centre`: one main box
/// of the given radius, then smaller offset part boxes, each tied back to the
/// centre by a hidden log stub. `radius ≤ 3` (the decay-support bound).
fn leaf_clump(
    ctx: &mut FeatureCtx,
    centre: IVec3,
    radius: i32,
    log: Block,
    leaf: Block,
    rng: &mut FeatureRng,
) {
    debug_assert!((2..=3).contains(&radius));
    let parts = rng.next_i32(2, 4);
    let main_half = IVec3::new(radius, (radius - 1).max(1), radius);
    clump_support(ctx, centre, main_half, log);
    leaf_box_eroded(ctx, centre, main_half, leaf, rng);
    for _ in 1..parts {
        let part_centre = IVec3::new(
            centre.x + rng.next_i32(-2, 2),
            centre.y + rng.next_i32(-1, 2),
            centre.z + rng.next_i32(-2, 2),
        );
        let part_half = IVec3::new(
            rng.next_i32((radius - 2).max(1), radius),
            rng.next_i32(1, (radius - 1).max(1)),
            rng.next_i32((radius - 2).max(1), radius),
        );
        log_line(ctx, centre, part_centre, log);
        clump_support(ctx, part_centre, part_half, log);
        leaf_box_eroded(ctx, part_centre, part_half, leaf, rng);
    }
}

/// One branch: a cardinal walk with scattered 1-block rises, an optional
/// elbow turn, a thickened first third (underside + one flank), and at most
/// one perpendicular split partway along. Tip cells (walk end + split end)
/// are appended to `tips` for the canopy pass. Coordinates are relative to
/// the feature origin so the `TIP_FENCE` bound is exact; the walk simply
/// stops at the fence (pure geometry, so a neighbour chunk replays it
/// identically).
#[allow(clippy::too_many_arguments)]
fn grow_branch(
    ctx: &mut FeatureCtx,
    origin: IVec3,
    start: (i32, i32, i32),
    dir: (i32, i32),
    length: i32,
    branch_index: i32,
    allow_split: bool,
    log: Block,
    tips: &mut Vec<(i32, i32, i32)>,
    rng: &mut FeatureRng,
) {
    let (mut x, mut y, mut z) = start;
    let (mut dx, mut dz) = dir;

    debug_assert!(length <= TIP_FENCE + 1);
    let rise_count = rng.next_i32(1, (length / 3).max(1));
    let mut rise_buf = [0i32; 16];
    let mut rise_len = 0;
    for step in 1..(length - 1).max(2) {
        rise_buf[rise_len] = step;
        rise_len += 1;
    }
    shuffle(&mut rise_buf[..rise_len], rng);
    let rise_steps = &rise_buf[..rise_len.min(rise_count as usize)];

    let mut turn_step = None;
    if length >= 6 && rng.chance(0.38) {
        turn_step = Some(rng.next_i32(length / 2, length - 2));
    }

    let perp = rotate(dir, branch_index % 2 == 0);
    let mut path = [(0i32, 0i32, 0i32); 16];
    let mut path_len = 0usize;

    for step in 0..length {
        if (x + dx).abs().max((z + dz).abs()) > TIP_FENCE {
            break;
        }
        x += dx;
        z += dz;
        ctx.set_branch(IVec3::new(origin.x + x, origin.y + y, origin.z + z), log);
        if rise_steps.contains(&step) {
            y += 1;
            ctx.set_branch(IVec3::new(origin.x + x, origin.y + y, origin.z + z), log);
        }
        if step < (length / 3).max(2) {
            ctx.set_branch(
                IVec3::new(origin.x + x, origin.y + y - 1, origin.z + z),
                log,
            );
            ctx.set_branch(
                IVec3::new(origin.x + x + perp.0, origin.y + y, origin.z + z + perp.1),
                log,
            );
        }
        path[path_len] = (x, y, z);
        path_len += 1;
        if turn_step == Some(step) {
            let clockwise = rng.chance(0.5);
            let d = rotate((dx, dz), clockwise);
            dx = d.0;
            dz = d.1;
        }
    }

    tips.push((x, y, z));

    if allow_split && length >= 6 && rng.chance(0.62) && path_len > 0 {
        let split_index = ((path_len as f32 * 0.62).floor() as usize)
            .max(1)
            .min(path_len - 1);
        let split_origin = path[split_index];
        let split_dir = rotate((dx, dz), rng.chance(0.5));
        let split_len = rng.next_i32(2, (length / 2).max(3));
        grow_branch(
            ctx,
            origin,
            split_origin,
            split_dir,
            split_len,
            branch_index + 1000,
            false,
            log,
            tips,
            rng,
        );
    }
}

/// Stylized concept oak — the tuned tree the 2026-07 iterations converged on:
/// a wandering multi-radius trunk (5×5 flare, 3×3 shaft, bare 1×1 top)
/// studded with bark bumps, a fan of grid-line surface roots, branch LEVELS
/// of cardinal limbs — each with scattered 1-block rises, an optional elbow
/// turn and one perpendicular split — and an eroded cuboid leaf clump on
/// every branch tip under a heavier trunk-top crown ringed by side clumps.
/// One struct expresses every oak size as data in `data::features`.
///
/// Determinism/seam contract: draw order is fixed (trunk levels, roots, each
/// level's branches in shuffled-cardinal order, then canopy clumps in tip
/// order) and no draw reads world content, so a neighbour chunk replays the
/// tree identically. Horizontal bounds: branch walks stop at `TIP_FENCE`, so
/// leaves stay ≤ `TIP_FENCE + 5 == proto::MARGIN` and wood ≤ `TIP_FENCE + 1`.
/// Leaf-decay support: every clump box carries hidden support wood
/// (`clump_support`) and erosion never strands a corner (`leaf_box_eroded`),
/// so no leaf exceeds the decay flood's log distance.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockyOakFeature {
    pub log: Block,
    pub leaf: Block,
    /// {min,max} trunk height.
    pub height: (i32, i32),
    /// {min,max} branch levels along the upper trunk.
    pub levels: (i32, i32),
    /// Minimum reach of any single branch.
    pub reach_min: i32,
    /// {min,max} of the per-TREE maximum branch reach; each branch draws its
    /// length in `reach_min..=<that>`. Keep `.1 ≤ TIP_FENCE`.
    pub reach_max: (i32, i32),
    /// {min,max} root count around the base flare.
    pub roots: (i32, i32),
    /// {min,max} of the per-tree root reach; each root draws its length in
    /// `max(2, reach - 3)..=reach`.
    pub root_reach: (i32, i32),
    /// Base leaf-clump radius (tip clumps vary ±1, the crown adds 1). Keep at
    /// 2: the crown's radius-3 clump is the decay-support ceiling.
    pub leaf_radius: i32,
}

/// One cell of the oak's trunk-and-roots base, in world coords.
enum BaseCell {
    /// Trunk core / root log — written unconditionally.
    Log(IVec3),
    /// Bark bump — written with the branch predicate.
    Bump(IVec3),
}

impl BlockyOakFeature {
    /// Draw the trunk-and-roots base — the shared RNG prefix of `generate`
    /// and `is_anchored`. Every base cell goes through `emit`; returns the
    /// trunk height and per-level trunk centres. Both callers consume the
    /// stream identically by construction, so the anchoring dry-run and the
    /// real placement always agree on the geometry.
    fn plan_base(
        &self,
        origin: IVec3,
        rng: &mut FeatureRng,
        emit: &mut dyn FnMut(BaseCell),
    ) -> (i32, [(i32, i32); 40]) {
        use std::f32::consts::TAU;
        debug_assert!(self.leaf_radius <= 2 && self.reach_max.1 <= TIP_FENCE);
        debug_assert!(
            self.root_reach.1 <= crate::worldgen::biome::MAX_TREE_SPACING_RADIUS,
            "root reach must stay inside the candidate window for the anchoring gate"
        );
        let h = sample_height(self.height, rng);
        debug_assert!(h <= 40, "trunk height must fit the stack scratch");

        // Trunk: the centre wanders one step every five levels (clamped to
        // ±1), flaring wide at the base and thinning toward the top; sparse
        // bark bumps stud the lower two thirds. Stack scratch: every chunk
        // within MARGIN replays the tree, so per-replay allocation is
        // measurable.
        let mut centres = [(0i32, 0i32); 40];
        let (mut cx, mut cz) = (0i32, 0i32);
        for level in 0..h {
            if level >= 5 && level < h - 4 && level % 5 == 0 && rng.chance(0.45) {
                let (dx, dz) = CARDINALS[rng.next_i32(0, 3) as usize];
                if (cx + dx).abs() <= 1 && (cz + dz).abs() <= 1 {
                    cx += dx;
                    cz += dz;
                }
            }
            centres[level as usize] = (cx, cz);
            let r = trunk_radius(level, h);
            for dx in -r..=r {
                for dz in -r..=r {
                    if r == 2 && dx.abs() == 2 && dz.abs() == 2 {
                        continue;
                    }
                    emit(BaseCell::Log(IVec3::new(
                        origin.x + cx + dx,
                        origin.y + level,
                        origin.z + cz + dz,
                    )));
                }
            }
            if level >= 2 && level < (h as f32 * 0.72).floor() as i32 && rng.chance(0.18) {
                let (dx, dz) = CARDINALS[rng.next_i32(0, 3) as usize];
                emit(BaseCell::Bump(IVec3::new(
                    origin.x + cx + dx * (r + 1),
                    origin.y + level,
                    origin.z + cz + dz * (r + 1),
                )));
            }
        }

        // Roots: evenly fanned, angle-jittered grid lines stepping down and
        // out from the trunk base, doubled at ground level and widened near
        // the trunk.
        let root_count = sample_height(self.roots, rng);
        let root_reach = sample_height(self.root_reach, rng);
        let (sx, sz) = centres[0];
        for i in 0..root_count {
            let angle = TAU * i as f32 / root_count as f32 + (rng.next_f32() * 0.36 - 0.18);
            let len = rng.next_i32((root_reach - 3).max(2), root_reach);
            let ex = sx + (angle.cos() * len as f32).round() as i32;
            let ez = sz + (angle.sin() * len as f32).round() as i32;
            let path = grid_line_2d(sx, sz, ex, ez);
            let last = (path.len() - 1).max(1);
            for (step, &(px, pz)) in path.iter().enumerate() {
                let t = step as f32 / last as f32;
                let ry = if t < 0.34 { 1 } else { 0 };
                emit(BaseCell::Log(IVec3::new(
                    origin.x + px,
                    origin.y + ry,
                    origin.z + pz,
                )));
                emit(BaseCell::Log(IVec3::new(
                    origin.x + px,
                    origin.y,
                    origin.z + pz,
                )));
                if t < 0.28 {
                    let wx = if ez == sz {
                        0
                    } else if ez > sz {
                        1
                    } else {
                        -1
                    };
                    let wz = if ex == sx {
                        0
                    } else if ex > sx {
                        -1
                    } else {
                        1
                    };
                    emit(BaseCell::Log(IVec3::new(
                        origin.x + px + wx,
                        origin.y,
                        origin.z + pz + wz,
                    )));
                }
            }
        }

        (h, centres)
    }
}

impl Feature for BlockyOakFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        let (h, centres) = self.plan_base(origin, rng, &mut |cell| match cell {
            BaseCell::Log(p) => ctx.set_log(p, self.log),
            BaseCell::Bump(p) => ctx.set_branch(p, self.log),
        });

        // Branch levels: evenly spaced fork heights along the upper trunk,
        // two or three shuffled-cardinal branches each (two at the lowest and
        // highest levels).
        let low = (h as f32 * 0.43).floor() as i32;
        let high = h - 3;
        let levels = sample_height(self.levels, rng);
        let max_reach = sample_height(self.reach_max, rng);
        let mut tips: Vec<(i32, i32, i32)> = Vec::new();
        let mut branch_index = 0;
        for level in 0..levels {
            let base = if levels == 1 {
                low
            } else {
                (low as f32 + (high - low) as f32 * level as f32 / (levels - 1) as f32).round()
                    as i32
            };
            let ly = (base + rng.next_i32(-1, 1)).clamp(low, high.max(low));
            let mut dirs = CARDINALS;
            shuffle(&mut dirs, rng);
            let branch_count = if level == 0 || level == levels - 1 {
                2
            } else {
                rng.next_i32(2, 3)
            };
            let (ccx, ccz) = centres[ly as usize];
            let r = trunk_radius(ly, h);
            for &(dx, dz) in dirs.iter().take(branch_count as usize) {
                let start = (ccx + dx * r, ly, ccz + dz * r);
                let length = rng.next_i32(self.reach_min, max_reach);
                grow_branch(
                    ctx,
                    origin,
                    start,
                    (dx, dz),
                    length,
                    branch_index,
                    true,
                    self.log,
                    &mut tips,
                    rng,
                );
                branch_index += 1;
            }
        }

        // Canopy: an eroded clump on every branch tip (radius jittered ±1), a
        // heavier crown clump over the trunk top — its always-kept centre
        // column is what buries the top log — and a ring of four side clumps
        // just below it.
        let radius = self.leaf_radius.max(2);
        for &(tx, ty, tz) in &tips {
            let jitter = [-1, 0, 0, 1][rng.next_i32(0, 3) as usize];
            let local = (radius + jitter).max(2);
            leaf_clump(
                ctx,
                IVec3::new(origin.x + tx, origin.y + ty, origin.z + tz),
                local,
                self.log,
                self.leaf,
                rng,
            );
        }
        let top_y = h - 1;
        let (tcx, tcz) = centres[top_y as usize];
        leaf_clump(
            ctx,
            IVec3::new(origin.x + tcx, origin.y + top_y + 1, origin.z + tcz),
            radius + 1,
            self.log,
            self.leaf,
            rng,
        );
        for (dx, dz) in CARDINALS {
            if rng.chance(0.8) {
                leaf_clump(
                    ctx,
                    IVec3::new(
                        origin.x + tcx + dx * 2,
                        origin.y + top_y,
                        origin.z + tcz + dz * 2,
                    ),
                    radius,
                    self.log,
                    self.leaf,
                    rng,
                );
            }
        }
    }

    /// Every ground-level cell of the base (trunk flare + root lines) must
    /// rest on ground: the column's surface may be at the cell's level (the
    /// log replaces the surface block) or one below (the log sits on it).
    /// One hanging cell rejects the whole tree — no floating trees, and the
    /// skipped `generate` is a placement-cost win on slopes.
    fn is_anchored(
        &self,
        surf: &mut dyn FnMut(i32, i32) -> i32,
        origin: IVec3,
        rng: FeatureRng,
    ) -> bool {
        let mut rng = rng;
        let mut anchored = true;
        self.plan_base(origin, &mut rng, &mut |cell| {
            if let BaseCell::Log(p) = cell {
                if anchored && p.y == origin.y && surf(p.x, p.z) < origin.y - 1 {
                    anchored = false;
                }
            }
        });
        anchored
    }
}

/// Skeleton broadleaf tree — the rounded canopy silhouette (storybook look):
/// a single trunk that forks into a few limbs, each limb
/// tipped with a rounded leaf clump, under a rounded central crown. The
/// overlapping clumps read as one irregular puffy canopy. One struct expresses
/// the round-canopied broadleaf species (birch, jungle) as data — width, limb
/// count, clump size — in `data::features`; oaks use the deliberately boxy
/// `BlockyOakFeature` instead.
///
/// Determinism/seam contract: draw order is fixed (height, then per-limb
/// [jitter, reach, rise, tip radius, blob rounds], then crown rounds), limbs
/// spread by golden-angle offsets from one base-angle draw, so a neighbour
/// chunk replays the tree identically. Horizontal footprint is bounded by
/// `reach.1 + tip_radius.1` — keep that ≤ `proto::MARGIN`. Leaf-decay support:
/// every clump is centred on its limb's tip log, so no leaf exceeds the decay
/// flood's log distance.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanopyTreeFeature {
    pub log: Block,
    pub leaf: Block,
    /// {min,max} trunk height (logs).
    pub height: (i32, i32),
    /// Fraction of the trunk below the first limb (0.5 = limbs on the top half).
    pub split: f32,
    /// {min,max} limb count.
    pub limbs: (i32, i32),
    /// {min,max} horizontal limb reach in blocks.
    pub reach: (i32, i32),
    /// {min,max} radius of each limb-tip leaf clump.
    pub tip_radius: (i32, i32),
    /// Radius of the central crown clump over the trunk top.
    pub crown_radius: i32,
    /// Corner-rounding chance for every clump (see `leaf_blob_rounded`).
    pub round: f32,
}

/// Golden angle (radians): successive limbs fan out evenly without aligning.
const GOLDEN_ANGLE: f32 = 2.399_963_1;

impl Feature for CanopyTreeFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        use std::f32::consts::TAU;
        let (x, y, z) = (origin.x, origin.y, origin.z);
        let height = sample_height(self.height, rng);
        let top = y + height - 1;
        let split_y = y + ((height as f32) * self.split).floor() as i32;

        for h in y..=top {
            ctx.set_log(IVec3::new(x, h, z), self.log);
        }

        // Limbs fork off the upper trunk, evenly spaced with jitter, fanning by
        // golden-angle steps from one random base angle.
        let limbs = sample_height(self.limbs, rng);
        let base_angle = rng.next_f32() * TAU;
        let span = (top - 1 - split_y).max(0);
        for i in 0..limbs {
            let t = if limbs > 1 {
                i as f32 / (limbs - 1) as f32
            } else {
                0.5
            };
            // The limb forks somewhere on the upper trunk, but its leaf clump
            // always lands at canopy height (top−1 .. top+1): the clumps overlap
            // each other and the crown into ONE puffy mass with lobes, instead
            // of scattering down the trunk.
            let node_y = (split_y + (span as f32 * t).round() as i32 + rng.next_i32(-1, 1))
                .clamp(split_y, top - 1);
            let angle = base_angle + i as f32 * GOLDEN_ANGLE;
            let reach = sample_height(self.reach, rng);
            let tip = IVec3::new(
                x + (angle.cos() * reach as f32).round() as i32,
                top - 1 + rng.next_i32(0, 2),
                z + (angle.sin() * reach as f32).round() as i32,
            );
            let tip_r = sample_height(self.tip_radius, rng);
            log_line(ctx, IVec3::new(x, node_y, z), tip, self.log);
            shapes::leaf_blob_rounded(ctx, tip, tip_r, self.leaf, self.round, rng);
        }

        // Rounded central crown over the trunk top ties the clumps together.
        shapes::leaf_blob_rounded(
            ctx,
            IVec3::new(x, top, z),
            self.crown_radius,
            self.leaf,
            self.round,
            rng,
        );
    }
}

/// Huge redwood: a flared multi-block trunk that tapers upward, long upper limbs
/// with leaf masses, and a narrow high crown. Materials are placeholders (oak
/// log/leaf) until dedicated redwood assets exist.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedwoodFeature {
    pub log: Block,
    pub leaf: Block,
    /// {min,max} of the nominal total height H.
    pub height: (i32, i32),
}

pub(crate) const REDWOOD_BASE_SUPPORT_REACH: i32 = 5;

/// Per-corner trim chance for the redwood whorl leaf masses — high enough that the
/// small (r=2) clumps read as rounded blobs rather than the solid cubes a plain
/// `leaf_blob` produces at that radius.
const REDWOOD_CANOPY_ROUND: f32 = 0.7;

fn disc_contains(dx: i32, dz: i32, radius: f32) -> bool {
    let fx = dx.abs() as f32 + 0.5;
    let fz = dz.abs() as f32 + 0.5;
    fx * fx + fz * fz <= radius * radius
}

pub(crate) fn redwood_base_trunk_contains(dx: i32, dz: i32) -> bool {
    disc_contains(dx, dz, redwood_trunk_radius(0, 1))
}

fn log_disc(ctx: &mut FeatureCtx, center: IVec3, radius: f32, log: Block) {
    let ri = radius.ceil() as i32;
    for dx in -ri..=ri {
        for dz in -ri..=ri {
            if disc_contains(dx, dz, radius) {
                ctx.set_log(IVec3::new(center.x + dx, center.y, center.z + dz), log);
            }
        }
    }
}

fn redwood_trunk_radius(level: i32, height: i32) -> f32 {
    let t = level as f32 / (height - 1).max(1) as f32;
    let stem = 0.80 + 2.35 * (1.0 - t).powf(0.85);
    let flare = if t < 0.22 {
        1.45 * (1.0 - t / 0.22).powf(1.7)
    } else {
        0.0
    };
    stem + flare
}

impl Feature for RedwoodFeature {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng) {
        use std::f32::consts::TAU;
        let (x, y, z) = (origin.x, origin.y, origin.z);
        let height = sample_height(self.height, rng);
        let spine_top = y + height - 1;

        for level in 0..height {
            log_disc(
                ctx,
                IVec3::new(x, y + level, z),
                redwood_trunk_radius(level, height),
                self.log,
            );
        }

        let crown_base = y + (height as f32 * 0.38).floor() as i32;
        let crown_span = (spine_top - crown_base - 2).max(1);
        let whorls = (height / 4).clamp(9, 14);
        for i in 0..whorls {
            let t = if whorls > 1 {
                i as f32 / (whorls - 1) as f32
            } else {
                0.0
            };
            let node_y =
                (crown_base + (crown_span as f32 * t).round() as i32 + rng.next_i32(-1, 1))
                    .clamp(crown_base, spine_top - 1);
            let angle = rng.next_f32() * TAU + i as f32 * 2.399_963_1;
            let reach_base = 7.0 - 4.5 * t;
            let reach = (reach_base.round() as i32 + rng.next_i32(-1, 1)).clamp(2, 7);
            let tip = IVec3::new(
                x + (angle.cos() * reach as f32).round() as i32,
                node_y + rng.next_i32(-1, 1) + if t > 0.72 { 1 } else { 0 },
                z + (angle.sin() * reach as f32).round() as i32,
            );
            log_line(ctx, IVec3::new(x, node_y, z), tip, self.log);
            shapes::leaf_blob_rounded(
                ctx,
                tip,
                if t > 0.78 { 3 } else { 2 },
                self.leaf,
                REDWOOD_CANOPY_ROUND,
                rng,
            );
        }

        let top_crown_base = (spine_top - 10).max(crown_base);
        for (k, r) in [
            (0, 4.0f32),
            (1, 4.0),
            (2, 3.5),
            (3, 3.0),
            (4, 3.0),
            (5, 2.5),
            (6, 2.0),
            (7, 2.0),
            (8, 1.5),
            (9, 1.0),
            (10, 0.75),
        ] {
            shapes::leaf_disc(ctx, IVec3::new(x, top_crown_base + k, z), r, self.leaf);
        }
    }
}
