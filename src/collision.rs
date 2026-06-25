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

/// Boundary epsilon (world units). The body is shrunk by this before its float edges meet
/// block faces, so a body flush on a voxel boundary — or a hair off from float error — is
/// not treated as overlapping. Matches the player collision constant it was extracted from.
const EPS: f32 = 1e-4;

/// How tall a step a *grounded* walking body (player, mob) auto-climbs without jumping —
/// half a block, so it walks up slabs / a model block's low ledge but not a full block.
pub const STEP_HEIGHT: f32 = 0.5;

/// The largest signed distance the body `[min, max]` may travel along `axis` (0=x, 1=y,
/// 2=z) toward `delta` before a collision box from `boxes_fn` stops it — the swept-AABB
/// core. Scans every cell the body sweeps through (nearest wins, so it never tunnels) and,
/// for each box the body overlaps on the two OTHER axes (the whole point of a *shape*
/// system — you can stand on a half-height block or pass the empty margin of an inset
/// one), clamps travel to that box's near face. Returns `delta` when nothing blocks.
pub fn sweep_axis<F>(min: [f32; 3], max: [f32; 3], axis: usize, delta: f32, boxes_fn: F) -> f32
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
    travel
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
    let mut mn = min;
    let mut mx = max;
    let mut moved = [0.0f32; 3];
    let mut hit = [false; 3];

    // Y first, so we land before sliding horizontally.
    let dy = vel[1] * dt;
    if dy != 0.0 {
        let ty = sweep_axis(mn, mx, 1, dy, &boxes_fn);
        mn[1] += ty;
        mx[1] += ty;
        moved[1] += ty;
        hit[1] = ty.abs() + 1e-6 < dy.abs();
    }
    let grounded = hit[1] && dy < 0.0;

    // Horizontal: step up only while grounded (and only over a `step_height` ledge).
    let step = if grounded { step_height } else { 0.0 };
    let (hmoved, hit_x, hit_z) = step_horizontal(mn, mx, vel[0] * dt, vel[2] * dt, step, &boxes_fn);
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
    // Normal slide.
    let (nx, nz) = slide_xz(min, max, dx, dz, &boxes_fn);
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
    let up = sweep_axis(min, max, 1, step_height, &boxes_fn);
    if up <= EPS {
        return normal;
    }
    let rmin = [min[0], min[1] + up, min[2]];
    let rmax = [max[0], max[1] + up, max[2]];
    let (sx, sz) = slide_xz(rmin, rmax, dx, dz, &boxes_fn);
    // Keep the step only if the raised slide got us meaningfully further horizontally.
    if sx * sx + sz * sz <= nx * nx + nz * nz + 1e-9 {
        return normal;
    }
    // Settle back down onto the ledge (never below where we started).
    let smin = [rmin[0] + sx, rmin[1], rmin[2] + sz];
    let smax = [rmax[0] + sx, rmax[1], rmax[2] + sz];
    let down = sweep_axis(smin, smax, 1, -up, &boxes_fn);
    (
        [sx, up + down, sz],
        sx.abs() + 1e-6 < dx.abs(),
        sz.abs() + 1e-6 < dz.abs(),
    )
}

/// Slide a body horizontally: sweep X, then sweep Z from the X-resolved position (so a wall
/// on one axis never blocks the other). Returns the per-axis travel.
fn slide_xz<F>(min: [f32; 3], max: [f32; 3], dx: f32, dz: f32, boxes_fn: &F) -> (f32, f32)
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    let tx = sweep_axis(min, max, 0, dx, boxes_fn);
    let m2 = [min[0] + tx, min[1], min[2]];
    let mx2 = [max[0] + tx, max[1], max[2]];
    let tz = sweep_axis(m2, mx2, 2, dz, boxes_fn);
    (tx, tz)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A full unit cube (a normal solid block's shape).
    const FULL: &[Aabb] = &[Aabb {
        min: [0.0; 3],
        max: [1.0; 3],
    }];
    /// A chest-style inset box: 1/16 margin on the sides, 14/16 tall.
    const INSET: &[Aabb] = &[Aabb {
        min: [0.0625, 0.0, 0.0625],
        max: [0.9375, 0.875, 0.9375],
    }];

    /// One solid cell at `(0,0,0)` of the given shape; everything else empty.
    fn one_cell(shape: &'static [Aabb]) -> impl Fn(i32, i32, i32) -> &'static [Aabb] {
        move |x, y, z| if (x, y, z) == (0, 0, 0) { shape } else { &[] }
    }

    #[test]
    fn sweep_stops_at_a_full_cube_face() {
        // A unit body at x∈[2,3] moving -X toward the cube at cell 0 (its +X face is x=1)
        // travels until its min meets x=1: from 2 to 1 → -1.0, not the requested -5.
        let travel = sweep_axis([2.0, 0.2, 0.2], [3.0, 0.8, 0.8], 0, -5.0, one_cell(FULL));
        assert!(
            (travel - (-1.0)).abs() < 1e-3,
            "stops at the cube face, got {travel}"
        );
        // With no cross-axis overlap (body above the cube) it passes freely.
        let free = sweep_axis([2.0, 5.0, 0.2], [3.0, 6.0, 0.8], 0, -5.0, one_cell(FULL));
        assert!(
            (free - (-5.0)).abs() < 1e-6,
            "no overlap → full travel, got {free}"
        );
    }

    #[test]
    fn sweep_respects_an_inset_box_margin() {
        // Falling onto the inset block: a body resting at y just above stops at the box top
        // (y = 0.875), not the cell top (y = 1.0).
        let travel = sweep_axis([0.3, 1.5, 0.3], [0.7, 2.5, 0.7], 1, -2.0, one_cell(INSET));
        assert!(
            (travel - (0.875 - 1.5)).abs() < 1e-3,
            "rests on the inset top, got {travel}"
        );
        // A thin body wholly inside the 1/16 side MARGIN (x ∈ [0, 0.05], left of the inset's
        // -X face at 0.0625) falls straight past it — no cross-axis overlap, full travel —
        // where a full cube would have stopped it.
        let margin = sweep_axis([0.0, 1.5, 0.3], [0.05, 2.5, 0.7], 1, -2.0, one_cell(INSET));
        assert!(
            (margin - (-2.0)).abs() < 1e-6,
            "falls through the side margin, got {margin}"
        );
    }

    #[test]
    fn resolve_body_lands_grounded_on_a_floor() {
        // A 0.4-wide, 0.6-tall body falling onto the floor cell (0,0,0): it lands on y=1
        // (the cube top) and reports grounded.
        let floor = |_x: i32, y: i32, _z: i32| if y == 0 { FULL } else { &[][..] };
        let (moved, grounded, hit) = resolve_body(
            [0.3, 1.4, 0.3],
            [0.7, 2.0, 0.7],
            [0.0, -5.0, 0.0],
            0.1,
            0.0,
            floor,
        );
        assert!(grounded, "a downward stop is grounded");
        assert!(hit[1] && !hit[0] && !hit[2], "only Y blocked");
        // Wanted -0.5; clamped so the body bottom (1.4) meets the floor top (1.0) → -0.4.
        assert!(
            (moved[1] - (-0.4)).abs() < 1e-3,
            "clamped to the floor, got {}",
            moved[1]
        );
    }

    #[test]
    fn step_horizontal_climbs_a_half_block_but_not_a_full_one() {
        // A 0.5-tall ledge at cell x=1 (a box world-y ∈ [1, 1.5]) plus the floor (y=0).
        let half_step = |_x: i32, y: i32, _z: i32| -> &'static [Aabb] {
            if y == 0 {
                FULL
            } else if y == 1 {
                &[Aabb {
                    min: [0.0, 0.0, 0.0],
                    max: [1.0, 0.5, 1.0],
                }]
            } else {
                &[]
            }
        };
        // Body standing on the floor at x∈[0.2,0.8], feet y=1, walking +X by 0.5 into x=1.
        let (moved, hit_x, _) = step_horizontal(
            [0.2, 1.0, 0.2],
            [0.8, 2.0, 0.8],
            0.5,
            0.0,
            STEP_HEIGHT,
            half_step,
        );
        assert!(!hit_x, "a 0.5 step is climbed, not blocked");
        assert!(moved[0] > 0.4, "it advanced over the step, dx={}", moved[0]);
        assert!(
            (moved[1] - 0.5).abs() < 0.05,
            "it rose onto the step top, dy={}",
            moved[1]
        );

        // A FULL block at cell x=1 (y∈[1,2]) is NOT climbed.
        let full_step = |_x: i32, y: i32, _z: i32| -> &'static [Aabb] {
            if y == 0 || y == 1 {
                FULL
            } else {
                &[]
            }
        };
        let (moved2, hit_x2, _) = step_horizontal(
            [0.2, 1.0, 0.2],
            [0.8, 2.0, 0.8],
            0.5,
            0.0,
            STEP_HEIGHT,
            full_step,
        );
        assert!(hit_x2, "a full block blocks");
        assert!(
            moved2[1] < 1e-3,
            "no rise over a full block, dy={}",
            moved2[1]
        );
        assert!(
            moved2[0] < 0.3,
            "only slid up to the wall face, dx={}",
            moved2[0]
        );

        // With step_height = 0 (the airborne / no-step case), even the 0.5 ledge blocks.
        let (moved3, hit_x3, _) =
            step_horizontal([0.2, 1.0, 0.2], [0.8, 2.0, 0.8], 0.5, 0.0, 0.0, half_step);
        assert!(hit_x3, "no step-up when not grounded");
        assert!(moved3[1] < 1e-3, "no rise when step disabled");
    }

    #[test]
    fn point_in_solid_respects_the_inset_margin() {
        // A point inside the inset box is solid; one in the 1/16 side margin is free.
        assert!(point_in_solid([0.5, 0.5, 0.5], one_cell(INSET)));
        assert!(
            !point_in_solid([0.02, 0.5, 0.5], one_cell(INSET)),
            "side margin is free"
        );
        // A point in an empty cell is never solid.
        assert!(!point_in_solid([0.5, 5.5, 0.5], one_cell(INSET)));
    }
}
