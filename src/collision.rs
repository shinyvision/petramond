//! The one model-aware voxel-collision primitive, shared by every moving entity — the
//! player, mobs, dropped items, and particles. There is no per-entity collision logic:
//! a body is just a world-space AABB `[min, max]`, a particle is a point, and the world
//! is described by `boxes_fn(x, y, z) -> &'static [Aabb]` — the cell's collision boxes
//! (empty = nothing to hit). The world feeds [`World::collision_boxes_at`] (which is
//! model-aware: a bbmodel block returns its real per-cell legs/top, a normal block its
//! full cube); tests feed a stub that maps a bool to `Block::Stone/Air.collision_boxes()`.
//! So everything collides with the actual block *shape*, and a new entity type collides
//! correctly for free.
//!
//! The resolver is a CONTINUOUS swept-AABB (no tunnelling, slides to the exact face),
//! lifted from the player's original `Player::sweep_boxes` so the player, mobs, and items
//! share one implementation; particles, being points, use the cheaper [`point_in_solid`].
//!
//! [`World::collision_boxes_at`]: crate::world::World::collision_boxes_at

use crate::block::Aabb;

#[cfg(test)]
mod tests;

/// A DYNAMIC world-space collision box — a solid entity's body (`mobs.json`
/// `"collision": "solid"`), resolved alongside the world's cell boxes by the
/// `*_dyn` entry points. `id` is the owning entity's stable id, so an entity
/// resolving its own movement can skip its own box (`ignore`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DynBox {
    pub id: u64,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl DynBox {
    /// The dynamic boxes that participate against the entity `ignore` —
    /// every box but its own.
    #[inline]
    fn against(dyn_boxes: &[DynBox], ignore: u64) -> impl Iterator<Item = &DynBox> {
        dyn_boxes.iter().filter(move |d| d.id != ignore)
    }
}

/// The `ignore` a NON-entity body passes to the `*_dyn` resolvers (the
/// player): matches no live entity id, so every dynamic box participates.
pub const NOT_AN_ENTITY: u64 = u64::MAX;

/// Boundary epsilon (world units). The body is shrunk by this before its float edges meet
/// block faces, so a body flush on a voxel boundary — or a hair off from float error — is
/// not treated as overlapping. Matches the player collision constant it was extracted from.
const EPS: f32 = 1e-4;

/// Largest per-tick displacement accepted from an external locomotion intent.
/// Sweeps are continuous, but their broad phase scans every crossed cell; this
/// bound keeps a hostile mod call from turning one entity step into an
/// effectively unbounded cell walk. Internal physics remains free to use the
/// resolver directly for distances it owns.
pub const MAX_SAFE_EXTERNAL_SWEEP_DISTANCE: f32 = 16.0;

/// Whether two open world-space AABBs overlap. Touching faces are not an
/// overlap, matching the swept resolver's contact semantics.
#[inline]
pub fn aabb_overlaps(
    min: [f32; 3],
    max: [f32; 3],
    other_min: [f32; 3],
    other_max: [f32; 3],
) -> bool {
    (0..3).all(|axis| min[axis] < other_max[axis] && max[axis] > other_min[axis])
}

/// Whether a world-space AABB overlaps any cell-local collision box in the
/// cells it spans. This is the neutral overlap query used by server claim
/// validation and both sides' riding placement probes.
pub fn aabb_hits_cells<F>(min: [f32; 3], max: [f32; 3], boxes_fn: F) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    for x in (min[0].floor() as i32)..=(max[0].floor() as i32) {
        for y in (min[1].floor() as i32)..=(max[1].floor() as i32) {
            for z in (min[2].floor() as i32)..=(max[2].floor() as i32) {
                let cell = [x as f32, y as f32, z as f32];
                for b in boxes_fn(x, y, z) {
                    let bmin = [cell[0] + b.min[0], cell[1] + b.min[1], cell[2] + b.min[2]];
                    let bmax = [cell[0] + b.max[0], cell[1] + b.max[1], cell[2] + b.max[2]];
                    if aabb_overlaps(min, max, bmin, bmax) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Whether an AABB overlaps any participating dynamic body.
pub fn aabb_hits_dynamic(min: [f32; 3], max: [f32; 3], dyn_boxes: &[DynBox], ignore: u64) -> bool {
    DynBox::against(dyn_boxes, ignore).any(|d| aabb_overlaps(min, max, d.min, d.max))
}

/// How tall a step a *grounded* walking body (player, mob) auto-climbs without jumping —
/// half a block, so it walks up slabs / a model block's low ledge but not a full block.
pub const STEP_HEIGHT: f32 = 0.5;

/// How far past a step allowance the sneak support probes reach: a support top
/// sitting EXACTLY a step below (the slab step-down) must pass the strict
/// interval tests despite float noise. Block geometry is 1/16-grained, so the
/// margin can never legalize the next-taller drop. Shared by
/// [`clamp_to_supported`] and the player's sneak snap-down, which must agree on
/// what counts as "within a step" or a move the clamp allowed could fail to
/// settle.
pub const SUPPORT_PROBE_MARGIN: f32 = 0.01;

/// The largest signed distance the body `[min, max]` may travel along `axis` (0=x, 1=y,
/// 2=z) toward `delta` before a collision box from `boxes_fn` stops it — the swept-AABB
/// core. Scans every cell the body sweeps through (nearest wins, so it never tunnels) and,
/// for each box the body overlaps on the two OTHER axes (the whole point of a *shape*
/// system — you can stand on a half-height block or pass the empty margin of an inset
/// one), clamps travel to that box's near face. Returns `delta` when nothing blocks.
/// World-only form of [`sweep_axis_dyn`] — production sweeps that may meet a
/// solid entity always pass the dynamic boxes, so this stays a test entry.
#[cfg(test)]
pub fn sweep_axis<F>(min: [f32; 3], max: [f32; 3], axis: usize, delta: f32, boxes_fn: F) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    sweep_axis_dyn(min, max, axis, delta, boxes_fn, &[], 0)
}

/// [`sweep_axis`] that ALSO clamps against dynamic world-space boxes (solid
/// entities), skipping the one owned by `ignore`.
pub fn sweep_axis_dyn<F>(
    min: [f32; 3],
    max: [f32; 3],
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
    // Broad-phase cell ranges over the swept volume: the body, with the swept axis
    // extended by `delta` toward the move direction.
    let mut lo = [
        min[0].floor() as i32,
        min[1].floor() as i32,
        min[2].floor() as i32,
    ];
    let mut hi = [
        max[0].floor() as i32,
        max[1].floor() as i32,
        max[2].floor() as i32,
    ];
    if delta > 0.0 {
        hi[ai] = (max[ai] + delta).floor() as i32;
    } else {
        lo[ai] = (min[ai] + delta).floor() as i32;
    }

    let mut travel = delta;
    for cx in lo[0]..=hi[0] {
        for cy in lo[1]..=hi[1] {
            for cz in lo[2]..=hi[2] {
                let cell = [cx as f32, cy as f32, cz as f32];
                for b in boxes_fn(cx, cy, cz) {
                    // Overlap on the two NON-swept axes (touching within EPS doesn't count).
                    let mut cross = true;
                    for i in 0..3 {
                        if i == ai {
                            continue;
                        }
                        let wlo = cell[i] + b.min[i];
                        let whi = cell[i] + b.max[i];
                        if !(max[i] > wlo + EPS && min[i] < whi - EPS) {
                            cross = false;
                            break;
                        }
                    }
                    if !cross {
                        continue;
                    }
                    // Clamp travel so the leading face just meets the box's near face on
                    // the swept axis (only while the box is ahead of us).
                    if delta > 0.0 {
                        let allowed = (cell[ai] + b.min[ai]) - max[ai];
                        if allowed >= -EPS {
                            travel = travel.min(allowed.max(0.0));
                        }
                    } else {
                        let allowed = (cell[ai] + b.max[ai]) - min[ai];
                        if allowed <= EPS {
                            travel = travel.max(allowed.min(0.0));
                        }
                    }
                }
            }
        }
    }
    // Dynamic boxes: the same clamp, world-space (no cell base).
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
    travel
}

/// Lift a body straight up out of SHALLOW foot penetration. Swept collision
/// deliberately ignores boxes a body already overlaps (that is what lets it
/// slide off contact without sticking), so when a block GROWS under standing
/// feet — a mod pressing 15/16 farmland back to full-cube dirt, a machine
/// variant swap — the next downward sweep would tunnel straight through the
/// floor. This pre-pass heals that: any box whose vertical span contains the
/// FEET line (and overlaps the body on X/Z) lifts the body onto its top,
/// capped by `max_lift` and clamped by the actual headroom above (via an
/// upward sweep, so a low ceiling gives a partial lift instead of clipping
/// the head). Boxes higher up in the body are not healed — you cannot climb
/// out of a block materialised at chest height. Returns the applied lift
/// (`0.0` almost always: a body flush ON a box top does not count as inside
/// it).
/// World-only form of [`depenetrate_up_dyn`] (see [`sweep_axis`]).
#[cfg(test)]
pub fn depenetrate_up<F>(min: [f32; 3], max: [f32; 3], max_lift: f32, boxes_fn: F) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    depenetrate_up_dyn(min, max, max_lift, boxes_fn, &[], 0)
}

/// [`depenetrate_up`] that also heals out of dynamic boxes — a solid entity
/// surfacing under standing feet (a boat rising beneath a swimmer) lifts the
/// body onto its top exactly like a grown block.
pub fn depenetrate_up_dyn<F>(
    min: [f32; 3],
    max: [f32; 3],
    max_lift: f32,
    boxes_fn: F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let mut need = 0.0f32;
    for cx in min[0].floor() as i32..=max[0].floor() as i32 {
        for cy in min[1].floor() as i32..=max[1].floor() as i32 {
            for cz in min[2].floor() as i32..=max[2].floor() as i32 {
                let cell = [cx as f32, cy as f32, cz as f32];
                for b in boxes_fn(cx, cy, cz) {
                    // Overlap on X/Z (touching within EPS doesn't count) —
                    // the same cross test the sweep uses.
                    let mut cross = true;
                    for i in [0, 2] {
                        let wlo = cell[i] + b.min[i];
                        let whi = cell[i] + b.max[i];
                        if !(max[i] > wlo + EPS && min[i] < whi - EPS) {
                            cross = false;
                            break;
                        }
                    }
                    if !cross {
                        continue;
                    }
                    let wbot = cell[1] + b.min[1];
                    let wtop = cell[1] + b.max[1];
                    if wbot <= min[1] + EPS && wtop > min[1] + EPS {
                        need = need.max(wtop - min[1]);
                    }
                }
            }
        }
    }
    for d in DynBox::against(dyn_boxes, ignore) {
        let cross = [0, 2]
            .iter()
            .all(|&i| max[i] > d.min[i] + EPS && min[i] < d.max[i] - EPS);
        if cross && d.min[1] <= min[1] + EPS && d.max[1] > min[1] + EPS {
            need = need.max(d.max[1] - min[1]);
        }
    }
    let need = need.min(max_lift);
    if need <= EPS {
        return 0.0;
    }
    // Respect headroom: the boxes being escaped sit behind an upward sweep
    // (their bottoms are below the head), so only a real ceiling clamps.
    sweep_axis_dyn(min, max, 1, need, boxes_fn, dyn_boxes, ignore).max(0.0)
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
#[cfg(test)]
pub fn clamp_to_supported<F>(
    min: [f32; 3],
    max: [f32; 3],
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

/// [`clamp_to_supported`] whose support band also sees dynamic boxes — a
/// sneaking body edge-guards on a solid entity's deck like on any floor.
#[allow(clippy::too_many_arguments)]
pub fn clamp_to_supported_dyn<F>(
    min: [f32; 3],
    max: [f32; 3],
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
    // The pull-back increment. Small enough that a clamped move still hugs the
    // edge lip closely; per-substep deltas are at most a few multiples of it.
    const STEP: f32 = 0.05;
    // Support: any collision box intersecting the band from `max_drop` below the
    // feet up to the feet line, under the horizontally-offset body. Strict at the
    // feet line so a wall RESTING at foot height (a step-up ahead) is not
    // mistaken for floor — the ordinary sweep handles walls.
    let supported = |ox: f32, oz: f32| -> bool {
        let lo = [
            min[0] + ox,
            min[1] - max_drop - SUPPORT_PROBE_MARGIN,
            min[2] + oz,
        ];
        let hi = [max[0] + ox, min[1], max[2] + oz];
        for cx in lo[0].floor() as i32..=hi[0].floor() as i32 {
            for cy in lo[1].floor() as i32..=hi[1].floor() as i32 {
                for cz in lo[2].floor() as i32..=hi[2].floor() as i32 {
                    let cell = [cx as f32, cy as f32, cz as f32];
                    for b in boxes_fn(cx, cy, cz) {
                        let inside = (0..3).all(|i| {
                            let wlo = cell[i] + b.min[i];
                            let whi = cell[i] + b.max[i];
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

/// Resolve a simple body's whole move for one tick: sweep Y (so it lands first), then the
/// horizontal move via [`step_horizontal`] — which auto-climbs a `step_height` ledge ONLY
/// when the body is grounded (resting on the floor), like a player/mob walking up a slab.
/// Pass `step_height = 0.0` for a body that should never step (a dropped item). Returns
/// `(moved, grounded, hit)` — the per-axis displacement, whether a downward Y move was
/// stopped (resting on ground), and which axes were blocked (the caller zeroes velocity on
/// those). Shared by mob + dropped-item physics; the player drives [`step_horizontal`] /
/// [`sweep_axis`] directly because it layers water on top.
pub fn resolve_body<F>(
    min: [f32; 3],
    max: [f32; 3],
    vel: [f32; 3],
    dt: f32,
    step_height: f32,
    boxes_fn: F,
) -> ([f32; 3], bool, [bool; 3])
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    resolve_body_dyn(min, max, vel, dt, step_height, boxes_fn, &[], 0)
}

/// [`resolve_body`] that also resolves against dynamic boxes — how a mob
/// collides with solid entities (and a solid entity with its peers, its own
/// box skipped via `ignore`).
#[allow(clippy::too_many_arguments)]
pub fn resolve_body_dyn<F>(
    min: [f32; 3],
    max: [f32; 3],
    vel: [f32; 3],
    dt: f32,
    step_height: f32,
    boxes_fn: F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> ([f32; 3], bool, [bool; 3])
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let mut mn = min;
    let mut mx = max;

    // Heal shallow foot penetration first (a block grew under the body —
    // see `depenetrate_up`), so the Y sweep lands ON the new top instead of
    // tunnelling through the box it started inside.
    let lift = depenetrate_up_dyn(mn, mx, STEP_HEIGHT, &boxes_fn, dyn_boxes, ignore);
    if lift > 0.0 {
        mn[1] += lift;
        mx[1] += lift;
    }

    let (mut moved, grounded, hit) = resolve_body_dyn_from_depenetrated(
        mn,
        mx,
        vel,
        dt,
        step_height,
        boxes_fn,
        dyn_boxes,
        ignore,
    );
    moved[1] += lift;
    (moved, grounded, hit)
}

/// Resolve a body known to have completed the shallow-foot depenetration
/// pre-pass. Kept crate-private for compound-body orchestration that must
/// preserve that mandatory lift as a separate motion waypoint.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_body_dyn_from_depenetrated<F>(
    mut mn: [f32; 3],
    mut mx: [f32; 3],
    vel: [f32; 3],
    dt: f32,
    step_height: f32,
    boxes_fn: F,
    dyn_boxes: &[DynBox],
    ignore: u64,
) -> ([f32; 3], bool, [bool; 3])
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let mut moved = [0.0f32; 3];
    let mut hit = [false; 3];

    // Y first, so we land before sliding horizontally.
    let dy = vel[1] * dt;
    if dy != 0.0 {
        let ty = sweep_axis_dyn(mn, mx, 1, dy, &boxes_fn, dyn_boxes, ignore);
        mn[1] += ty;
        mx[1] += ty;
        moved[1] += ty;
        hit[1] = ty.abs() + 1e-6 < dy.abs();
    }
    let grounded = hit[1] && dy < 0.0;

    // Horizontal: step up only while grounded (and only over a `step_height` ledge).
    let step = if grounded { step_height } else { 0.0 };
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

/// Resolve a horizontal move `(dx, dz)` for a body at `[min, max]`, with optional STEP-UP.
/// Returns `(moved, hit_x, hit_z)` — the net `[dx, dy, dz]` displacement (`dy > 0` when it
/// stepped up) and whether each horizontal axis was still blocked.
///
/// First slides normally (X then Z). If that's blocked and `step_height > 0`, it retries
/// the slide lifted by up to `step_height` (ceiling-capped) and, if that advances further,
/// settles back down onto the ledge — the classic auto-step over a slab / low edge. An
/// obstacle taller than `step_height` still blocks (the lifted body can't advance), so a
/// full block is never climbed. `step_height = 0.0` is a plain slide. The caller gates
/// `step_height` on being grounded.
pub fn step_horizontal<F>(
    min: [f32; 3],
    max: [f32; 3],
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

/// [`step_horizontal`] that also slides against (and steps onto) dynamic
/// boxes.
#[allow(clippy::too_many_arguments)]
pub fn step_horizontal_dyn<F>(
    min: [f32; 3],
    max: [f32; 3],
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
    // Normal slide.
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

    // Try stepping: how high can we rise (capped by a ceiling)?
    let up = sweep_axis_dyn(min, max, 1, step_height, &boxes_fn, dyn_boxes, ignore);
    if up <= EPS {
        return normal;
    }
    let rmin = [min[0], min[1] + up, min[2]];
    let rmax = [max[0], max[1] + up, max[2]];
    let (sx, sz) = slide_xz(rmin, rmax, dx, dz, &boxes_fn, dyn_boxes, ignore);
    // Keep the step only if the raised slide got us meaningfully further horizontally.
    if sx * sx + sz * sz <= nx * nx + nz * nz + 1e-9 {
        return normal;
    }
    // Settle back down onto the ledge (never below where we started).
    let smin = [rmin[0] + sx, rmin[1], rmin[2] + sz];
    let smax = [rmax[0] + sx, rmax[1], rmax[2] + sz];
    let down = sweep_axis_dyn(smin, smax, 1, -up, &boxes_fn, dyn_boxes, ignore);
    (
        [sx, up + down, sz],
        sx.abs() + 1e-6 < dx.abs(),
        sz.abs() + 1e-6 < dz.abs(),
    )
}

/// Slide a body horizontally: sweep X, then sweep Z from the X-resolved position (so a wall
/// on one axis never blocks the other). Returns the per-axis travel.
fn slide_xz<F>(
    min: [f32; 3],
    max: [f32; 3],
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
    let m2 = [min[0] + tx, min[1], min[2]];
    let mx2 = [max[0] + tx, max[1], max[2]];
    let tz = sweep_axis_dyn(m2, mx2, 2, dz, boxes_fn, dyn_boxes, ignore);
    (tx, tz)
}

/// The farthest a point may travel from `start` along unit `dir` (up to `max_dist`)
/// while keeping `pad` clearance from every collision box — the third-person camera
/// boom. Equivalent to sweeping a `2·pad` cube along the segment: each box is expanded
/// by `pad` (Minkowski) and the segment is slab-clipped against it; the nearest entry
/// distance wins. Returns `max_dist` when nothing blocks, and `0.0` when the start is
/// already inside an expanded box (the camera stays at the eye rather than clipping).
pub fn clamp_padded_segment<F>(
    start: [f32; 3],
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
    let end = [
        start[0] + dir[0] * max_dist,
        start[1] + dir[1] * max_dist,
        start[2] + dir[2] * max_dist,
    ];
    // Broad phase: every cell the padded segment's AABB touches.
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
                let cell = [cx as f32, cy as f32, cz as f32];
                for b in boxes_fn(cx, cy, cz) {
                    // Slab-clip the ray against the pad-expanded box.
                    let mut t_enter = 0.0f32;
                    let mut t_exit = travel;
                    let mut miss = false;
                    for i in 0..3 {
                        let bmin = cell[i] + b.min[i] - pad;
                        let bmax = cell[i] + b.max[i] + pad;
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
    travel
}

/// Is point `p` inside any collision box of its cell? The particle test — a particle is a
/// point, not a body, so it stops the instant it enters a real box (a leg/top), passing
/// through the empty margin of an inset/model cell. EPS keeps a point exactly on a face
/// from counting, matching [`sweep_axis`]. Cell-local: boxes never extend past their cell
/// (model boxes are clipped per cell; a normal block's box *is* its cell).
pub fn point_in_solid<F>(p: [f32; 3], boxes_fn: F) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let cell = [
        p[0].floor() as i32,
        p[1].floor() as i32,
        p[2].floor() as i32,
    ];
    let base = [cell[0] as f32, cell[1] as f32, cell[2] as f32];
    for b in boxes_fn(cell[0], cell[1], cell[2]) {
        if (0..3).all(|i| p[i] > base[i] + b.min[i] + EPS && p[i] < base[i] + b.max[i] - EPS) {
            return true;
        }
    }
    false
}

