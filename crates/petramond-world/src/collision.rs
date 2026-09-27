use crate::block::Aabb;

#[cfg(test)]
mod tests;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DynBox {
    pub id: u64,
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl DynBox {
    #[inline]
    fn against(dyn_boxes: &[DynBox], ignore: u64) -> impl Iterator<Item = &DynBox> {
        dyn_boxes.iter().filter(move |d| d.id != ignore)
    }
}

pub const NOT_AN_ENTITY: u64 = u64::MAX;

const EPS: f64 = 1e-4;

pub const MAX_SAFE_EXTERNAL_SWEEP_DISTANCE: f32 = 16.0;

#[inline]
fn at_cell(cell: i32, local: f32) -> f64 {
    f64::from(cell) + f64::from(local)
}

#[inline]
pub fn aabb_overlaps(
    min: [f64; 3],
    max: [f64; 3],
    other_min: [f64; 3],
    other_max: [f64; 3],
) -> bool {
    (0..3).all(|axis| min[axis] < other_max[axis] && max[axis] > other_min[axis])
}

pub fn aabb_hits_cells<F>(min: [f64; 3], max: [f64; 3], boxes_fn: F) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    for x in (min[0].floor() as i32)..=(max[0].floor() as i32) {
        for y in (min[1].floor() as i32)..=(max[1].floor() as i32) {
            for z in (min[2].floor() as i32)..=(max[2].floor() as i32) {
                let cell = [x, y, z];
                for b in boxes_fn(x, y, z) {
                    let bmin: [f64; 3] = std::array::from_fn(|i| at_cell(cell[i], b.min[i]));
                    let bmax: [f64; 3] = std::array::from_fn(|i| at_cell(cell[i], b.max[i]));
                    if aabb_overlaps(min, max, bmin, bmax) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub fn aabb_hits_dynamic(min: [f64; 3], max: [f64; 3], dyn_boxes: &[DynBox], ignore: u64) -> bool {
    DynBox::against(dyn_boxes, ignore).any(|d| aabb_overlaps(min, max, d.min, d.max))
}

pub const STEP_HEIGHT: f32 = 0.5;

pub const SUPPORT_PROBE_MARGIN: f32 = 0.01;

#[cfg(any(test, feature = "test-support"))]
pub fn sweep_axis<F>(min: [f64; 3], max: [f64; 3], axis: usize, delta: f32, boxes_fn: F) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    sweep_axis_dyn(min, max, axis, delta, boxes_fn, &[], 0)
}

pub fn sweep_axis_dyn<F>(
    min: [f64; 3],
    max: [f64; 3],
    axis: usize,
    delta: f32,
    boxes_fn: F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    if delta == 0.0 {
        return 0.0;
    }
    let ai = axis;
    let reach = f64::from(delta);
    let mut lo = min.map(|v| v.floor() as i32);
    let mut hi = max.map(|v| v.floor() as i32);
    if delta > 0.0 {
        hi[ai] = (max[ai] + reach).floor() as i32;
    } else {
        lo[ai] = (min[ai] + reach).floor() as i32;
    }

    let mut travel = reach;
    for cx in lo[0]..=hi[0] {
        for cy in lo[1]..=hi[1] {
            for cz in lo[2]..=hi[2] {
                let cell = [cx, cy, cz];
                for b in boxes_fn(cx, cy, cz) {
                    let mut cross = true;
                    for i in 0..3 {
                        if i == ai {
                            continue;
                        }
                        let wlo = at_cell(cell[i], b.min[i]);
                        let whi = at_cell(cell[i], b.max[i]);
                        if !(max[i] > wlo + EPS && min[i] < whi - EPS) {
                            cross = false;
                            break;
                        }
                    }
                    if !cross {
                        continue;
                    }
                    if delta > 0.0 {
                        let allowed = at_cell(cell[ai], b.min[ai]) - max[ai];
                        if allowed >= -EPS {
                            travel = travel.min(allowed.max(0.0));
                        }
                    } else {
                        let allowed = at_cell(cell[ai], b.max[ai]) - min[ai];
                        if allowed <= EPS {
                            travel = travel.max(allowed.min(0.0));
                        }
                    }
                }
            }
        }
    }
    for d in DynBox::against(dyn_boxes, ignore) {
        let mut cross = true;
        for i in 0..3 {
            if i == ai {
                continue;
            }
            if !(max[i] > d.min[i] + EPS && min[i] < d.max[i] - EPS) {
                cross = false;
                break;
            }
        }
        if !cross {
            continue;
        }
        if delta > 0.0 {
            let allowed = d.min[ai] - max[ai];
            if allowed >= -EPS {
                travel = travel.min(allowed.max(0.0));
            }
        } else {
            let allowed = d.max[ai] - min[ai];
            if allowed <= EPS {
                travel = travel.max(allowed.min(0.0));
            }
        }
    }
    travel as f32
}

pub type BodyBox = ([f64; 3], [f64; 3]);

pub const ESCAPE_SPEED: f32 = 2.0;

pub const BORE_SPEED: f32 = 8.0;

const ESCAPE_RADIUS: f64 = 3.0;

const ESCAPE_PATH_SAMPLE: f64 = 0.25;

/// An escape no longer than this is taken in ONE tick rather than squeezed
/// along: a body standing on a block that grew under its feet is a step's
/// worth of overlap and must be out of it before the same tick's downward
/// sweep runs, or it tunnels through the floor it is standing on. Longer
/// routes are a visible slide (see [`ESCAPE_SPEED`]).
const ESCAPE_SNAP: f32 = STEP_HEIGHT;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Escape {
    Free,
    Route([f32; 3]),
    Bore([f32; 3]),
}

fn visit_overlaps<F>(
    body: &[BodyBox],
    off: [f64; 3],
    boxes_fn: &F,
    dyn_boxes: &[DynBox],
    ignore: u64,
    mut f: impl FnMut(BodyBox) -> bool,
) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    for &(min, max) in body {
        let min: [f64; 3] = std::array::from_fn(|i| min[i] + off[i] + EPS);
        let max: [f64; 3] = std::array::from_fn(|i| max[i] + off[i] - EPS);
        for cx in min[0].floor() as i32..=max[0].floor() as i32 {
            for cy in min[1].floor() as i32..=max[1].floor() as i32 {
                for cz in min[2].floor() as i32..=max[2].floor() as i32 {
                    let cell = [cx, cy, cz];
                    for b in boxes_fn(cx, cy, cz) {
                        let lo: [f64; 3] = std::array::from_fn(|i| at_cell(cell[i], b.min[i]));
                        let hi: [f64; 3] = std::array::from_fn(|i| at_cell(cell[i], b.max[i]));
                        if aabb_overlaps(min, max, lo, hi) && f((lo, hi)) {
                            return true;
                        }
                    }
                }
            }
        }
        for d in DynBox::against(dyn_boxes, ignore) {
            if aabb_overlaps(min, max, d.min, d.max) && f((d.min, d.max)) {
                return true;
            }
        }
    }
    false
}

/// Whether the body overlaps nothing at `off`.
fn pose_is_free<F>(
    body: &[BodyBox],
    off: [f64; 3],
    boxes_fn: &F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    !visit_overlaps(body, off, boxes_fn, dyn_boxes, ignore, |_| true)
}

/// The shortest way out of geometry the body is already inside.
///
/// Swept collision deliberately ignores boxes a body already overlaps — that
/// is what lets it slide along contact instead of sticking — so a body that
/// ends up INSIDE one (a trunk grew around it, a door shut on it, a block was
/// placed on it, terrain streamed in under a claim) has no ordinary way back
/// out: every sweep passes straight through the box it started in.
///
/// The search is over POSES, never over axes: a shortest-penetration guess is
/// only a way out when the place it points at is empty, and in a one-wide slot
/// it points at the opposite wall. Candidates are tried nearest-first and each
/// is accepted only when the destination overlaps nothing AND the straight
/// path there passes through nothing the body is not already inside (so an
/// escape can never pop a body through a wall into the pocket beyond it).
///
/// Candidates come in two rounds. First the exits of the boxes the body is
/// actually in: the distance that clears every one of them along each of the
/// six directions, and the combinations of those — minimal moves that keep
/// the body's alignment, which is what frees a body pressed into a corner or
/// a floor. Then, when none of those land anywhere empty, the canonical
/// standing poses of the nearby cells (centred, feet on the cell floor),
/// which is what frees a body whose own alignment does not fit anywhere.
///
/// `Sealed` means the body is entombed; the engine reports that and holds it
/// still, because what should happen to it is a gameplay decision.
pub fn escape_pose<F>(body: &[BodyBox], boxes_fn: &F, dyn_boxes: &[DynBox], ignore: u64) -> Escape
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let mut inside: Vec<BodyBox> = Vec::new();
    visit_overlaps(body, [0.0; 3], boxes_fn, dyn_boxes, ignore, |b| {
        if !inside.contains(&b) {
            inside.push(b);
        }
        false
    });
    if inside.is_empty() {
        return Escape::Free;
    }

    let mut need = [0.0f64; 6];
    for &(min, max) in body {
        for &(lo, hi) in &inside {
            if !aabb_overlaps(min, max, lo, hi) {
                continue;
            }
            for axis in 0..3 {
                need[axis * 2] = need[axis * 2].max(hi[axis] - min[axis] + EPS);
                need[axis * 2 + 1] = need[axis * 2 + 1].max(max[axis] - lo[axis] + EPS);
            }
        }
    }

    let mut candidates: Vec<[f64; 3]> = Vec::new();
    for sx in -1i32..=1 {
        for sy in -1i32..=1 {
            for sz in -1i32..=1 {
                let signs = [sx, sy, sz];
                if signs == [0, 0, 0] {
                    continue;
                }
                candidates.push(std::array::from_fn(|axis| match signs[axis] {
                    1 => need[axis * 2],
                    -1 => -need[axis * 2 + 1],
                    _ => 0.0,
                }));
            }
        }
    }
    sort_by_distance(&mut candidates);
    if let Some(off) = first_usable(&candidates, body, &inside, boxes_fn, dyn_boxes, ignore) {
        return Escape::Route(off.map(|v| v as f32));
    }
    let exits = candidates.clone();

    let mut umin = body[0].0;
    let mut umax = body[0].1;
    for &(min, max) in &body[1..] {
        for axis in 0..3 {
            umin[axis] = umin[axis].min(min[axis]);
            umax[axis] = umax[axis].max(max[axis]);
        }
    }
    let here = [
        (umin[0] + umax[0]) * 0.5,
        umin[1],
        (umin[2] + umax[2]) * 0.5,
    ];
    let reach = ESCAPE_RADIUS.ceil() as i32;
    let base = here.map(|v| v.floor() as i32);
    candidates.clear();
    for dx in -reach..=reach {
        for dy in -reach..=reach {
            for dz in -reach..=reach {
                let cell = [base[0] + dx, base[1] + dy, base[2] + dz];
                let target = [
                    f64::from(cell[0]) + 0.5,
                    f64::from(cell[1]) + EPS,
                    f64::from(cell[2]) + 0.5,
                ];
                candidates.push(std::array::from_fn(|axis| target[axis] - here[axis]));
            }
        }
    }
    sort_by_distance(&mut candidates);
    if let Some(off) = first_usable(&candidates, body, &inside, boxes_fn, dyn_boxes, ignore) {
        return Escape::Route(off.map(|v| v as f32));
    }

    for round in [&exits[..], &candidates[..]] {
        let bore = round.iter().copied().find(|&off| {
            length_squared(off) > EPS && pose_is_free(body, off, boxes_fn, dyn_boxes, ignore)
        });
        if let Some(off) = bore {
            return Escape::Bore(off.map(|v| v as f32));
        }
    }
    Escape::Bore([0.0, 1.0, 0.0])
}

fn sort_by_distance(candidates: &mut [[f64; 3]]) {
    candidates.sort_by(|a, b| {
        let (da, db) = (length_squared(*a), length_squared(*b));
        da.total_cmp(&db)
            .then_with(|| a[0].total_cmp(&b[0]))
            .then_with(|| a[1].total_cmp(&b[1]))
            .then_with(|| a[2].total_cmp(&b[2]))
    });
}

#[inline]
fn length_squared(v: [f64; 3]) -> f64 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

/// The first candidate whose destination is free and whose straight path only
/// crosses `inside` — the geometry the body is already in. Beyond the point
/// the body gets free the path must STAY free: a route that leaves one box
/// and enters another has tunnelled, however empty its far end is.
fn first_usable<F>(
    candidates: &[[f64; 3]],
    body: &[BodyBox],
    inside: &[BodyBox],
    boxes_fn: &F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> Option<[f64; 3]>
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    candidates.iter().copied().find(|&off| {
        let len = length_squared(off).sqrt();
        if len <= EPS || !pose_is_free(body, off, boxes_fn, dyn_boxes, ignore) {
            return false;
        }
        let steps = (len / ESCAPE_PATH_SAMPLE).ceil() as i32;
        let mut freed = false;
        for i in 1..steps {
            let t = f64::from(i) / f64::from(steps);
            let at = off.map(|v| v * t);
            let mut touched_any = false;
            let hit_new = visit_overlaps(body, at, boxes_fn, dyn_boxes, ignore, |b| {
                touched_any = true;
                !inside.contains(&b)
            });
            if !touched_any {
                freed = true;
            } else if freed || hit_new {
                return false;
            }
        }
        true
    })
}

/// A body's committed way out (see [`escape_pose`]), held across ticks.
///
/// The plan is committed on purpose. Re-deciding every tick is what makes a
/// body oscillate: as it slides, the nearest way out changes under it, and a
/// route that takes more than one tick never completes. So the route is
/// planned once, walked at [`ESCAPE_SPEED`], and re-planned only when the
/// world moves the destination out from under it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EscapeRoute {
    remaining: [f32; 3],
    boring: bool,
}

impl EscapeRoute {
    /// Still on its way out of geometry: this tick's offset didn't finish the route. Don't sweep a
    /// body like that, because a sweep ignores the boxes it already overlaps and gravity just drags
    /// it back down. Drivers give the tick to the escape instead of moving it themselves.
    pub fn in_progress(&self) -> bool {
        self.remaining != [0.0; 3]
    }

    pub fn entombed(&self) -> bool {
        self.boring
    }

    pub fn advance<F>(
        &mut self,
        body: &[BodyBox],
        dt: f32,
        boxes_fn: &F,
        dyn_boxes: &[DynBox],
        ignore: u64,
    ) -> [f32; 3]
    where
        F: Fn(i32, i32, i32) -> &'static [Aabb],
    {
        if pose_is_free(body, [0.0; 3], boxes_fn, dyn_boxes, ignore) {
            *self = Self::default();
            return [0.0; 3];
        }
        let keep = !self.boring
            && self.remaining != [0.0; 3]
            && pose_is_free(
                body,
                self.remaining.map(f64::from),
                boxes_fn,
                dyn_boxes,
                ignore,
            );
        if !keep {
            match escape_pose(body, boxes_fn, dyn_boxes, ignore) {
                Escape::Free => {
                    *self = Self::default();
                    return [0.0; 3];
                }
                Escape::Route(off) => {
                    *self = Self {
                        remaining: off,
                        boring: false,
                    }
                }
                Escape::Bore(off) => {
                    *self = Self {
                        remaining: off,
                        boring: true,
                    }
                }
            }
        }
        let len = (length_squared(self.remaining.map(f64::from)).sqrt()) as f32;
        let fraction = if !self.boring && len <= ESCAPE_SNAP {
            1.0
        } else {
            let speed = if self.boring {
                BORE_SPEED
            } else {
                ESCAPE_SPEED
            };
            (speed * dt / len).min(1.0)
        };
        let off = self.remaining.map(|v| v * fraction);
        self.remaining = std::array::from_fn(|axis| self.remaining[axis] - off[axis]);
        off
    }
}

/// Shrink a horizontal move `(dx, dz)` so the body `[min, max]` keeps solid support
/// within `max_drop` below its feet at the destination — the sneak edge guard. A
/// drop within `max_drop` (stepping down a slab, the mirror of the auto step-up)
/// passes; a destination whose support band is empty (walking off a ledge) has the
/// offending axis pulled back toward zero in small increments, so the body slides
/// along the edge lip instead of over it. Axes are checked independently first,
/// then combined, so a diagonal move keeps its along-the-edge component. A body
/// that is ALREADY unsupported (mid-air callers) is left alone — the caller gates
/// on being grounded.
/// World-only form of [`clamp_to_supported_dyn`] (see [`sweep_axis`]).
#[cfg(any(test, feature = "test-support"))]
pub fn clamp_to_supported<F>(
    min: [f64; 3],
    max: [f64; 3],
    dx: f32,
    dz: f32,
    max_drop: f32,
    boxes_fn: F,
) -> (f32, f32)
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    clamp_to_supported_dyn(min, max, dx, dz, max_drop, boxes_fn, &[], 0)
}

#[allow(clippy::too_many_arguments)]
pub fn clamp_to_supported_dyn<F>(
    min: [f64; 3],
    max: [f64; 3],
    dx: f32,
    dz: f32,
    max_drop: f32,
    boxes_fn: F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> (f32, f32)
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    const STEP: f32 = 0.05;
    let supported = |ox: f32, oz: f32| -> bool {
        let (ox, oz) = (f64::from(ox), f64::from(oz));
        let lo = [
            min[0] + ox,
            min[1] - f64::from(max_drop) - f64::from(SUPPORT_PROBE_MARGIN),
            min[2] + oz,
        ];
        let hi = [max[0] + ox, min[1], max[2] + oz];
        for cx in lo[0].floor() as i32..=hi[0].floor() as i32 {
            for cy in lo[1].floor() as i32..=hi[1].floor() as i32 {
                for cz in lo[2].floor() as i32..=hi[2].floor() as i32 {
                    let cell = [cx, cy, cz];
                    for b in boxes_fn(cx, cy, cz) {
                        let inside = (0..3).all(|i| {
                            let wlo = at_cell(cell[i], b.min[i]);
                            let whi = at_cell(cell[i], b.max[i]);
                            hi[i] > wlo + EPS && lo[i] < whi - EPS
                        });
                        if inside {
                            return true;
                        }
                    }
                }
            }
        }
        DynBox::against(dyn_boxes, ignore)
            .any(|d| (0..3).all(|i| hi[i] > d.min[i] + EPS && lo[i] < d.max[i] - EPS))
    };
    if !supported(0.0, 0.0) {
        return (dx, dz);
    }
    let shrink = |v: f32| {
        if v.abs() <= STEP {
            0.0
        } else {
            v - STEP * v.signum()
        }
    };
    let (mut cx, mut cz) = (dx, dz);
    while cx != 0.0 && !supported(cx, 0.0) {
        cx = shrink(cx);
    }
    while cz != 0.0 && !supported(0.0, cz) {
        cz = shrink(cz);
    }
    while cx != 0.0 && cz != 0.0 && !supported(cx, cz) {
        cx = shrink(cx);
        cz = shrink(cz);
    }
    (cx, cz)
}

/// Moves a simple body for one tick. Y goes first so it lands, then [`step_horizontal`] handles
/// the rest and climbs a `step_height` ledge if the body is grounded. Dropped items pass
/// `step_height = 0.0` so they never step. Returns `(moved, grounded, hit)`; the caller zeroes
/// velocity on the blocked axes. The player skips this and calls [`step_horizontal`] and
/// `sweep_axis` directly, since it adds water on top.
pub fn resolve_body<F>(
    min: [f64; 3],
    max: [f64; 3],
    vel: [f32; 3],
    dt: f32,
    step_height: f32,
    route: &mut EscapeRoute,
    boxes_fn: F,
) -> ([f32; 3], bool, [bool; 3])
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    resolve_body_dyn(min, max, vel, dt, step_height, route, boxes_fn, &[], 0)
}

#[allow(clippy::too_many_arguments)]
pub fn resolve_body_dyn<F>(
    min: [f64; 3],
    max: [f64; 3],
    vel: [f32; 3],
    dt: f32,
    step_height: f32,
    route: &mut EscapeRoute,
    boxes_fn: F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> ([f32; 3], bool, [bool; 3])
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let mut mn = min;
    let mut mx = max;
    let (escape, escaping) =
        escape_pre_pass(&mut mn, &mut mx, dt, route, &boxes_fn, dyn_boxes, ignore);
    if escaping {
        return (escape, false, [false; 3]);
    }

    let (mut moved, grounded, hit) = resolve_body_dyn_escaped(
        mn,
        mx,
        vel,
        dt,
        step_height,
        false,
        boxes_fn,
        dyn_boxes,
        ignore,
    );
    for axis in 0..3 {
        moved[axis] += escape[axis];
    }
    (moved, grounded, hit)
}

pub fn escape_pre_pass<F>(
    mn: &mut [f64; 3],
    mx: &mut [f64; 3],
    dt: f32,
    route: &mut EscapeRoute,
    boxes_fn: &F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> ([f32; 3], bool)
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let off = route.advance(&[(*mn, *mx)], dt, boxes_fn, dyn_boxes, ignore);
    for axis in 0..3 {
        mn[axis] += f64::from(off[axis]);
        mx[axis] += f64::from(off[axis]);
    }
    (off, route.in_progress())
}

#[allow(clippy::too_many_arguments)]
pub fn resolve_body_dyn_escaped<F>(
    mut mn: [f64; 3],
    mut mx: [f64; 3],
    vel: [f32; 3],
    dt: f32,
    step_height: f32,
    step_supported: bool,
    boxes_fn: F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> ([f32; 3], bool, [bool; 3])
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let mut moved = [0.0f32; 3];
    let mut hit = [false; 3];

    let dy = vel[1] * dt;
    if dy != 0.0 {
        let ty = sweep_axis_dyn(mn, mx, 1, dy, &boxes_fn, dyn_boxes, ignore);
        mn[1] += f64::from(ty);
        mx[1] += f64::from(ty);
        moved[1] += ty;
        hit[1] = ty.abs() + 1e-6 < dy.abs();
    }
    let grounded = hit[1] && dy < 0.0;

    let step = if grounded || step_supported {
        step_height
    } else {
        0.0
    };
    let (hmoved, hit_x, hit_z) = step_horizontal_dyn(
        mn,
        mx,
        vel[0] * dt,
        vel[2] * dt,
        step,
        &boxes_fn,
        dyn_boxes,
        ignore,
    );
    moved[0] += hmoved[0];
    moved[1] += hmoved[1];
    moved[2] += hmoved[2];
    hit[0] = hit_x;
    hit[2] = hit_z;

    (moved, grounded, hit)
}

pub fn step_horizontal<F>(
    min: [f64; 3],
    max: [f64; 3],
    dx: f32,
    dz: f32,
    step_height: f32,
    boxes_fn: F,
) -> ([f32; 3], bool, bool)
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    step_horizontal_dyn(min, max, dx, dz, step_height, boxes_fn, &[], 0)
}

#[allow(clippy::too_many_arguments)]
pub fn step_horizontal_dyn<F>(
    min: [f64; 3],
    max: [f64; 3],
    dx: f32,
    dz: f32,
    step_height: f32,
    boxes_fn: F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> ([f32; 3], bool, bool)
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let (nx, nz) = slide_xz(min, max, dx, dz, &boxes_fn, dyn_boxes, ignore);
    let blocked = nx.abs() + 1e-6 < dx.abs() || nz.abs() + 1e-6 < dz.abs();
    let normal = (
        [nx, 0.0, nz],
        nx.abs() + 1e-6 < dx.abs(),
        nz.abs() + 1e-6 < dz.abs(),
    );
    if !blocked || step_height <= 0.0 {
        return normal;
    }

    let up = sweep_axis_dyn(min, max, 1, step_height, &boxes_fn, dyn_boxes, ignore);
    if f64::from(up) <= EPS {
        return normal;
    }
    let rise = f64::from(up);
    let rmin = [min[0], min[1] + rise, min[2]];
    let rmax = [max[0], max[1] + rise, max[2]];
    let (sx, sz) = slide_xz(rmin, rmax, dx, dz, &boxes_fn, dyn_boxes, ignore);
    if sx * sx + sz * sz <= nx * nx + nz * nz + 1e-9 {
        return normal;
    }
    let smin = [rmin[0] + f64::from(sx), rmin[1], rmin[2] + f64::from(sz)];
    let smax = [rmax[0] + f64::from(sx), rmax[1], rmax[2] + f64::from(sz)];
    let down = sweep_axis_dyn(smin, smax, 1, -up, &boxes_fn, dyn_boxes, ignore);
    (
        [sx, up + down, sz],
        sx.abs() + 1e-6 < dx.abs(),
        sz.abs() + 1e-6 < dz.abs(),
    )
}

fn slide_xz<F>(
    min: [f64; 3],
    max: [f64; 3],
    dx: f32,
    dz: f32,
    boxes_fn: &F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> (f32, f32)
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let tx = sweep_axis_dyn(min, max, 0, dx, boxes_fn, dyn_boxes, ignore);
    let m2 = [min[0] + f64::from(tx), min[1], min[2]];
    let mx2 = [max[0] + f64::from(tx), max[1], max[2]];
    let tz = sweep_axis_dyn(m2, mx2, 2, dz, boxes_fn, dyn_boxes, ignore);
    (tx, tz)
}

/// Camera boom in third person. To keep the camera `pad` off the walls we grow each box by `pad`
/// and take the first one the segment hits. Starting inside one gives 0.0, so the camera just stays
/// at the eye.
pub fn clamp_padded_segment<F>(
    start: [f64; 3],
    dir: [f32; 3],
    max_dist: f32,
    pad: f32,
    boxes_fn: F,
) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    if max_dist <= 0.0 {
        return 0.0;
    }
    let dir = dir.map(f64::from);
    let (max_dist, pad) = (f64::from(max_dist), f64::from(pad));
    let end: [f64; 3] = std::array::from_fn(|i| start[i] + dir[i] * max_dist);
    let mut lo = [0i32; 3];
    let mut hi = [0i32; 3];
    for i in 0..3 {
        lo[i] = (start[i].min(end[i]) - pad).floor() as i32;
        hi[i] = (start[i].max(end[i]) + pad).floor() as i32;
    }

    let mut travel = max_dist;
    for cx in lo[0]..=hi[0] {
        for cy in lo[1]..=hi[1] {
            for cz in lo[2]..=hi[2] {
                let cell = [cx, cy, cz];
                for b in boxes_fn(cx, cy, cz) {
                    let mut t_enter = 0.0f64;
                    let mut t_exit = travel;
                    let mut miss = false;
                    for i in 0..3 {
                        let bmin = at_cell(cell[i], b.min[i]) - pad;
                        let bmax = at_cell(cell[i], b.max[i]) + pad;
                        if dir[i].abs() < 1e-8 {
                            if start[i] < bmin || start[i] > bmax {
                                miss = true;
                                break;
                            }
                            continue;
                        }
                        let inv = 1.0 / dir[i];
                        let (t0, t1) = {
                            let a = (bmin - start[i]) * inv;
                            let b = (bmax - start[i]) * inv;
                            if a < b {
                                (a, b)
                            } else {
                                (b, a)
                            }
                        };
                        t_enter = t_enter.max(t0);
                        t_exit = t_exit.min(t1);
                        if t_enter > t_exit {
                            miss = true;
                            break;
                        }
                    }
                    if !miss {
                        travel = travel.min(t_enter.max(0.0));
                    }
                }
            }
        }
    }
    travel as f32
}

pub fn point_in_solid<F>(p: [f64; 3], boxes_fn: F) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let cell = p.map(|v| v.floor() as i32);
    for b in boxes_fn(cell[0], cell[1], cell[2]) {
        if (0..3).all(|i| {
            p[i] > at_cell(cell[i], b.min[i]) + EPS && p[i] < at_cell(cell[i], b.max[i]) - EPS
        }) {
            return true;
        }
    }
    false
}
