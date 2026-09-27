use petramond_math::detmath;
use petramond_world::{block::Block, mathh::IVec3};

use super::posture::{connected_branch, TrunkPosture};
use super::{rotate, shuffle, CARDINALS};
use crate::feature::placers::trunk::sample_height;
use crate::{
    feature::{Feature, FeatureCtx},
    rng::FeatureRng,
};

pub const TIP_FENCE: i32 = crate::feature::MARGIN - 5;

pub const TRUNK_HEIGHT: (i32, i32) = (7, 40);
pub const MAX_BRANCH_LEVELS: i32 = TRUNK_HEIGHT.1;
pub const MAX_ROOTS: i32 = 40;
pub const MIN_ROOT_REACH: i32 = 2;
/// Base leaf-clump radius. Tip clumps jitter ±1 around it, crown adds one more, so the crown's
/// radius-3 clump lands right at the leaf-decay support ceiling. Don't make this a row parameter;
/// any other value strands leaves.
pub const LEAF_RADIUS: i32 = 2;

const CORNER_ERODE_CHANCE: f32 = 0.72;
const EDGE_ERODE_CHANCE: f32 = 0.28;
const LONG_BRANCH: i32 = 6;
const BRANCH_TURN_CHANCE: f32 = 0.38;
const BRANCH_FORK_CHANCE: f32 = 0.62;
const BRANCH_FORK_AT: f32 = 0.62;
const BARK_BUMP_BAND: f32 = 0.72;
const BARK_BUMP_CHANCE: f32 = 0.18;
const ROOT_ANGLE_JITTER: f32 = 0.18;
const ROOT_RISE_FRACTION: f32 = 0.34;
const ROOT_WIDEN_FRACTION: f32 = 0.28;
const BRANCH_BAND_START: f32 = 0.43;
const CROWN_SIDE_CLUMP_CHANCE: f32 = 0.8;

fn trunk_radius(level: i32, height: i32) -> i32 {
    if level < (height / 7).max(3) {
        2
    } else if level < height - 3 {
        1
    } else {
        0
    }
}

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

/// Eroded leaf cuboid: corners drop 72% of the time, edges 28%, so it's blocky but not a perfect
/// box. The mask is built first, with no world reads.
/// A surviving corner with all three inward neighbours dropped gets dropped too, or it'd hang on
/// diagonally and decay.
/// Only overwrites Air/Water.
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
    let mut keep = [false; 7 * 7 * 7];
    for dx in -hx..=hx {
        for dy in -hy..=hy {
            for dz in -hz..=hz {
                let exposed =
                    (dx.abs() == hx) as i32 + (dy.abs() == hy) as i32 + (dz.abs() == hz) as i32;
                if exposed == 3 && rng.chance(CORNER_ERODE_CHANCE) {
                    continue;
                }
                if exposed == 2 && rng.chance(EDGE_ERODE_CHANCE) {
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

fn clump_support(ctx: &mut FeatureCtx, centre: IVec3, half: IVec3, log: Block) {
    ctx.set_branch(centre, log);
    if half.x >= 3 {
        for dx in [-2, -1, 1, 2] {
            ctx.set_branch(IVec3::new(centre.x + dx, centre.y, centre.z), log);
        }
    }
    if half.z >= 3 {
        for dz in [-2, -1, 1, 2] {
            ctx.set_branch(IVec3::new(centre.x, centre.y, centre.z + dz), log);
        }
    }
}

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
        connected_branch(ctx, centre, part_centre, log);
        clump_support(ctx, part_centre, part_half, log);
        leaf_box_eroded(ctx, part_centre, part_half, leaf, rng);
    }
}

#[allow(clippy::too_many_arguments)]
fn grow_branch(
    ctx: &mut FeatureCtx,
    origin: IVec3,
    start: (i32, i32, i32),
    dir: (i32, i32),
    length: i32,
    branch_index: i32,
    rise: (f32, f32),
    allow_split: bool,
    log: Block,
    tips: &mut Vec<(i32, i32, i32)>,
    rng: &mut FeatureRng,
) {
    let (mut x, mut y, mut z) = start;
    let (mut dx, mut dz) = dir;

    debug_assert!(length <= TIP_FENCE + 1);
    let rise_fraction = rise.0 + (rise.1 - rise.0) * rng.next_f32();
    let rise_count = (length as f32 * rise_fraction).round().max(1.0) as i32;
    let mut rise_buf = [0i32; 16];
    let mut rise_len = 0;
    for step in 1..(length - 1).max(2) {
        rise_buf[rise_len] = step;
        rise_len += 1;
    }
    shuffle(&mut rise_buf[..rise_len], rng);
    let rise_steps = &rise_buf[..rise_len.min(rise_count as usize)];

    let mut turn_step = None;
    if length >= LONG_BRANCH && rng.chance(BRANCH_TURN_CHANCE) {
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

    if allow_split && length >= LONG_BRANCH && rng.chance(BRANCH_FORK_CHANCE) && path_len > 0 {
        let split_index = ((path_len as f32 * BRANCH_FORK_AT).floor() as usize)
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
            rise,
            false,
            log,
            tips,
            rng,
        );
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockyOakFeature {
    pub log: Block,
    pub leaf: Block,
    pub height: (i32, i32),
    pub lean: (i32, i32),
    pub branch_rise: (f32, f32),
    pub crown_taper: f32,
    pub levels: (i32, i32),
    pub reach_min: i32,
    pub reach_max: (i32, i32),
    pub roots: (i32, i32),
    pub root_reach: (i32, i32),
}

impl BlockyOakFeature {
    pub fn validate(&self) -> Result<(), String> {
        use crate::data::bounds::{ascending, unit, unit_range, within};
        ascending("height", self.height, TRUNK_HEIGHT.0..=TRUNK_HEIGHT.1)?;
        within("reach_min", self.reach_min, 1..=self.reach_max.0)?;
        ascending("reach_max", self.reach_max, 1..=TIP_FENCE)?;
        ascending(
            "root_reach",
            self.root_reach,
            MIN_ROOT_REACH..=crate::biome::trees::MAX_TREE_SPACING_RADIUS,
        )?;
        ascending("levels", self.levels, 1..=MAX_BRANCH_LEVELS)?;
        ascending("roots", self.roots, 1..=MAX_ROOTS)?;
        ascending("lean", self.lean, 0..=TIP_FENCE - LEAF_RADIUS)?;
        unit_range("branch_rise", self.branch_rise)?;
        unit("crown_taper", self.crown_taper)
    }
}

enum BaseCell {
    Log(IVec3),
    Bump(IVec3),
}

impl BlockyOakFeature {
    fn plan_base(
        &self,
        origin: IVec3,
        rng: &mut FeatureRng,
        emit: &mut dyn FnMut(BaseCell),
    ) -> (i32, [(i32, i32); TRUNK_HEIGHT.1 as usize]) {
        use std::f32::consts::TAU;
        debug_assert!(self.validate().is_ok(), "rows are validated at load");
        let h = sample_height(self.height, rng);

        let posture = TrunkPosture::sample(self.lean, rng);
        let mut centres = [(0i32, 0i32); TRUNK_HEIGHT.1 as usize];
        for level in 0..h {
            let (cx, cz) = posture.offset(level, h);
            if level > 0 {
                let (mut px, mut pz) = centres[level as usize - 1];
                while px != cx {
                    px += (cx - px).signum();
                    emit(BaseCell::Log(IVec3::new(
                        origin.x + px,
                        origin.y + level - 1,
                        origin.z + pz,
                    )));
                }
                while pz != cz {
                    pz += (cz - pz).signum();
                    emit(BaseCell::Log(IVec3::new(
                        origin.x + px,
                        origin.y + level - 1,
                        origin.z + pz,
                    )));
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
            if level >= 2
                && level < (h as f32 * BARK_BUMP_BAND).floor() as i32
                && rng.chance(BARK_BUMP_CHANCE)
            {
                let (dx, dz) = CARDINALS[rng.next_i32(0, 3) as usize];
                emit(BaseCell::Bump(IVec3::new(
                    origin.x + cx + dx * (r + 1),
                    origin.y + level,
                    origin.z + cz + dz * (r + 1),
                )));
            }
        }

        let root_count = sample_height(self.roots, rng);
        let root_reach = sample_height(self.root_reach, rng);
        let (sx, sz) = centres[0];
        for i in 0..root_count {
            let angle = TAU * i as f32 / root_count as f32
                + (rng.next_f32() * 2.0 - 1.0) * ROOT_ANGLE_JITTER;
            let len = rng.next_i32((root_reach - 3).max(MIN_ROOT_REACH), root_reach);
            let ex = sx + (detmath::cosf(angle) * len as f32).round() as i32;
            let ez = sz + (detmath::sinf(angle) * len as f32).round() as i32;
            let path = grid_line_2d(sx, sz, ex, ez);
            let last = (path.len() - 1).max(1);
            for (step, &(px, pz)) in path.iter().enumerate() {
                let t = step as f32 / last as f32;
                let ry = if t < ROOT_RISE_FRACTION { 1 } else { 0 };
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
                if t < ROOT_WIDEN_FRACTION {
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
    fn generate(
        &self,
        ctx: &mut FeatureCtx,
        _open: &mut dyn FnMut(IVec3) -> bool,
        origin: IVec3,
        rng: &mut FeatureRng,
    ) {
        let (h, centres) = self.plan_base(origin, rng, &mut |cell| match cell {
            BaseCell::Log(p) => ctx.set_log(p, self.log),
            BaseCell::Bump(p) => ctx.set_branch(p, self.log),
        });

        let low = (h as f32 * BRANCH_BAND_START).floor() as i32;
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
                let crown_fraction = level as f32 / (levels - 1).max(1) as f32;
                let reach =
                    (max_reach as f32 * (1.0 - self.crown_taper * crown_fraction)).round() as i32;
                let length = rng.next_i32(self.reach_min, reach.max(self.reach_min));
                grow_branch(
                    ctx,
                    origin,
                    start,
                    (dx, dz),
                    length,
                    branch_index,
                    self.branch_rise,
                    true,
                    self.log,
                    &mut tips,
                    rng,
                );
                branch_index += 1;
            }
        }

        for &(tx, ty, tz) in &tips {
            let jitter = [-1, 0, 0, 1][rng.next_i32(0, 3) as usize];
            let local = (LEAF_RADIUS + jitter).max(LEAF_RADIUS);
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
            LEAF_RADIUS + 1,
            self.log,
            self.leaf,
            rng,
        );
        for (dx, dz) in CARDINALS {
            if rng.chance(CROWN_SIDE_CLUMP_CHANCE) {
                connected_branch(
                    ctx,
                    IVec3::new(origin.x + tcx, origin.y + top_y, origin.z + tcz),
                    IVec3::new(
                        origin.x + tcx + dx * 2,
                        origin.y + top_y,
                        origin.z + tcz + dz * 2,
                    ),
                    self.log,
                );
                leaf_clump(
                    ctx,
                    IVec3::new(
                        origin.x + tcx + dx * 2,
                        origin.y + top_y,
                        origin.z + tcz + dz * 2,
                    ),
                    LEAF_RADIUS,
                    self.log,
                    self.leaf,
                    rng,
                );
            }
        }
    }

    /// Every ground-level cell of the base (trunk flare and root lines) has to rest on ground.
    /// Surface can be at the cell's level (log replaces it) or one below (log sits on it). One
    /// hanging cell and we reject the whole tree, no floating trees. Bailing early here also saves
    /// a generate call on slopes.
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
