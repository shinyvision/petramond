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
//! World-space boxes and points are `f64`, like `WorldPos`: the contact epsilon and
//! the 1/16-grained block geometry stay meaningful however far out a body is.
//! Displacements and the travel a resolver returns stay `f32`.
//!
//! [`World::collision_boxes_at`]: crate::world::WorldData::collision_boxes_at

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
    pub min: [f64; 3],
    pub max: [f64; 3],
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

/// Boundary epsilon (world units). The body is shrunk by this before its edges meet
/// block faces, so a body flush on a voxel boundary — or a hair off from float error — is
/// not treated as overlapping. Matches the player collision constant it was extracted from.
const EPS: f64 = 1e-4;

/// Largest per-tick displacement accepted from an external locomotion intent.
/// Sweeps are continuous, but their broad phase scans every crossed cell; this
/// bound keeps a hostile mod call from turning one entity step into an
/// effectively unbounded cell walk. Internal physics remains free to use the
/// resolver directly for distances it owns.
pub const MAX_SAFE_EXTERNAL_SWEEP_DISTANCE: f32 = 16.0;

/// The world-space corner of cell `(x, y, z)` plus a cell-local offset.
#[inline]
fn at_cell(cell: i32, local: f32) -> f64 {
    f64::from(cell) + f64::from(local)
}

/// Whether two open world-space AABBs overlap. Touching faces are not an
/// overlap, matching the swept resolver's contact semantics.
#[inline]
pub fn aabb_overlaps(
    min: [f64; 3],
    max: [f64; 3],
    other_min: [f64; 3],
    other_max: [f64; 3],
) -> bool {
    (0..3).all(|axis| min[axis] < other_max[axis] && max[axis] > other_min[axis])
}

/// Whether a world-space AABB overlaps any cell-local collision box in the
/// cells it spans. This is the neutral overlap query used by server claim
/// validation and both sides' riding placement probes.
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

/// Whether an AABB overlaps any participating dynamic body.
pub fn aabb_hits_dynamic(min: [f64; 3], max: [f64; 3], dyn_boxes: &[DynBox], ignore: u64) -> bool {
    DynBox::against(dyn_boxes, ignore).any(|d| aabb_overlaps(min, max, d.min, d.max))
}

/// How tall a step a *grounded* walking body (player, mob) auto-climbs without jumping —
/// half a block, so it walks up slabs / a model block's low ledge but not a full block.
pub const STEP_HEIGHT: f32 = 0.5;

/// How far past a step allowance the sneak support probes reach: a support top
/// sitting EXACTLY a step below (the slab step-down) must pass the strict
/// interval tests despite float noise. Block geometry is 1/16-grained, so the
/// margin can never legalize the next-taller drop. Shared by
/// `clamp_to_supported` and the player's sneak snap-down, which must agree on
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
#[cfg(any(test, feature = "test-support"))]
pub fn sweep_axis<F>(min: [f64; 3], max: [f64; 3], axis: usize, delta: f32, boxes_fn: F) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [Aabb],
{
    sweep_axis_dyn(min, max, axis, delta, boxes_fn, &[], 0)
}

/// `sweep_axis` that ALSO clamps against dynamic world-space boxes (solid
/// entities), skipping the one owned by `ignore`.
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
    // Broad-phase cell ranges over the swept volume: the body, with the swept axis
    // extended by `delta` toward the move direction.
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
                    // Overlap on the two NON-swept axes (touching within EPS doesn't count).
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
                    // Clamp travel so the leading face just meets the box's near face on
                    // the swept axis (only while the box is ahead of us).
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
    travel as f32
}

/// A body as the escape search sees it: one world-space AABB per segment
/// (`(min, max)`). A simple body is a single box; a long one its whole run.
pub type BodyBox = ([f64; 3], [f64; 3]);

/// How fast (m/s) a body squeezes along its escape route. At the fixed tick
/// this is 0.1 m per tick — a visible slide out, never a teleport.
pub const ESCAPE_SPEED: f32 = 2.0;

/// How fast (m/s) a body BORES its way out when there is no clean way out at
/// all (see [`Escape::Bore`]). Faster than the squeeze: this is not a body
/// being nudged free of a door, it is a body buried in rock, and every tick
/// of it is a tick the player is not playing.
pub const BORE_SPEED: f32 = 8.0;

/// How far from its current pose the search looks for somewhere free. Three
/// blocks covers being shut in a door, walled into a one-block pocket, or
/// grown into by a trunk. Nothing free in reach does NOT mean "give up" — it
/// means bore (see [`Escape::Bore`]).
const ESCAPE_RADIUS: f64 = 3.0;

/// Spacing of the route validity samples (see [`escape_pose`]).
const ESCAPE_PATH_SAMPLE: f64 = 0.25;

/// An escape no longer than this is taken in ONE tick rather than squeezed
/// along: a body standing on a block that grew under its feet is a step's
/// worth of overlap and must be out of it before the same tick's downward
/// sweep runs, or it tunnels through the floor it is standing on. Longer
/// routes are a visible slide (see [`ESCAPE_SPEED`]).
const ESCAPE_SNAP: f32 = STEP_HEIGHT;

/// What a body inside geometry can do about it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Escape {
    /// Nothing to escape: the body overlaps no collision box.
    Free,
    /// The shortest offset to a pose that overlaps nothing, reachable without
    /// passing through geometry the body is not already inside.
    Route([f32; 3]),
    /// No clean way out: the shortest offset to a free pose REGARDLESS of
    /// what the way there passes through — or, when nothing free is in reach
    /// at all, one block straight up.
    ///
    /// A body buried in rock has no clean way out by definition, and that is
    /// the ordinary case underground, not an exotic one. Holding it still
    /// there is the same as losing it: the body must keep making progress
    /// toward open air, and boring is the only progress available. It still
    /// stops the moment a pose is genuinely free, and a body that is boring
    /// reports itself [`entombed`](EscapeRoute::entombed) the whole way, so
    /// gameplay can charge for it.
    Bore([f32; 3]),
}

/// Every world box (cell-local boxes of the cells the body spans, plus the
/// participating dynamic bodies) that `body` overlaps when translated by
/// `off`. `f` returning true stops the walk; the return value is whether it
/// did — so `visit_overlaps(.., |_| true)` is "does this pose overlap
/// anything".
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
        // Shrunk by the contact epsilon, like every other overlap test here:
        // a body flush against a face — or a hair inside one after a sweep
        // clamped to it — is touching, not stuck, and must not set the whole
        // search going every tick.
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
    // The boxes the body starts inside: both what it must clear and the only
    // geometry a route is allowed to travel through.
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

    // Travel that clears every one of them, per direction (+x, -x, +y, -y, +z, -z).
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

    // Round two: stand in a nearby cell instead of keeping our alignment.
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
                // The canonical standing pose of that cell: centred on it,
                // feet a hair above its floor.
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

    // No clean way out. Take the nearest free pose anyway and bore toward it
    // — through rock if that is what stands between the body and open air.
    // Every candidate from both rounds is back in play, judged on its
    // destination alone.
    for round in [&exits[..], &candidates[..]] {
        let bore = round.iter().copied().find(|&off| {
            length_squared(off) > EPS && pose_is_free(body, off, boxes_fn, dyn_boxes, ignore)
        });
        if let Some(off) = bore {
            return Escape::Bore(off.map(|v| v as f32));
        }
    }
    // Nothing free within reach either: head for the sky, which always is.
    Escape::Bore([0.0, 1.0, 0.0])
}

/// Nearest first, with a fixed tie-break so two identical worlds always pick
/// the same way out (the client predicts this and the server verifies it).
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
    /// Whether the body is still on its way out of geometry: this tick's
    /// offset did not finish the route. Sweeping such a body is meaningless
    /// — a sweep ignores the boxes it already overlaps, so gravity simply
    /// drags it back down whatever the escape gained — so its drivers give
    /// the tick to the escape and skip their own motion.
    pub fn in_progress(&self) -> bool {
        self.remaining != [0.0; 3]
    }

    /// Whether the body is ENTOMBED: inside geometry with no clean way out,
    /// boring toward open air (see [`Escape::Bore`]). A gameplay fact, not a
    /// physics one — the engine reports it and keeps the body moving; what a
    /// body pays for being buried is a mod's decision.
    pub fn entombed(&self) -> bool {
        self.boring
    }

    /// Advance the route by one tick of `dt` and return the offset to apply
    /// (zero only when the body is free).
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
        // A committed route is kept while its destination stands — re-deciding
        // every tick is what oscillates. A BORING body is the exception: it is
        // travelling through solid, so every tick it should ask again whether
        // a clean way out has come into reach.
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
        // A clean escape shorter than a step happens at once (a heal, not a
        // squeeze). Boring is never instant: it passes through solid, so it
        // stays a visible, interruptible crawl however short the hop.
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

/// `clamp_to_supported` whose support band also sees dynamic boxes — a
/// sneaking body edge-guards on a solid entity's deck like on any floor.
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
    // The pull-back increment. Small enough that a clamped move still hugs the
    // edge lip closely; per-substep deltas are at most a few multiples of it.
    const STEP: f32 = 0.05;
    // Support: any collision box intersecting the band from `max_drop` below the
    // feet up to the feet line, under the horizontally-offset body. Strict at the
    // feet line so a wall RESTING at foot height (a step-up ahead) is not
    // mistaken for floor — the ordinary sweep handles walls.
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

/// Resolve a simple body's whole move for one tick: sweep Y (so it lands first), then the
/// horizontal move via [`step_horizontal`] — which auto-climbs a `step_height` ledge ONLY
/// when the body is grounded (resting on the floor), like a player/mob walking up a slab.
/// Pass `step_height = 0.0` for a body that should never step (a dropped item). Returns
/// `(moved, grounded, hit)` — the per-axis displacement, whether a downward Y move was
/// stopped (resting on ground), and which axes were blocked (the caller zeroes velocity on
/// those). Shared by mob + dropped-item physics; the player drives [`step_horizontal`] /
/// `sweep_axis` directly because it layers water on top.
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

/// [`resolve_body`] that also resolves against dynamic boxes — how a mob
/// collides with solid entities (and a solid entity with its peers, its own
/// box skipped via `ignore`).
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
        // Still inside geometry: the escape owns this tick (see `in_progress`).
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

/// The pre-pass every moving body runs before its sweeps: get OUT of geometry
/// it is already inside, because no sweep will (they ignore boxes the body
/// already overlaps).
///
/// It always makes progress. A body with a clean way out walks its
/// [`EscapeRoute`]; a body buried in rock bores toward the nearest open air,
/// and failing that toward the sky. Nothing here can leave a body standing
/// still inside geometry — that is the one outcome a stuck player cannot
/// recover from. Displaces `[mn, mx]` and returns the applied offset.
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

/// Resolve a body known to have completed the escape pre-pass (and to be
/// out of geometry — see [`escape_pre_pass`]). `step_supported` also permits stepping without a solid landing.
/// Used by compound-body orchestration that must
/// preserve that mandatory lift as a separate motion waypoint.
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

    // Y first, so we land before sliding horizontally.
    let dy = vel[1] * dt;
    if dy != 0.0 {
        let ty = sweep_axis_dyn(mn, mx, 1, dy, &boxes_fn, dyn_boxes, ignore);
        mn[1] += f64::from(ty);
        mx[1] += f64::from(ty);
        moved[1] += ty;
        hit[1] = ty.abs() + 1e-6 < dy.abs();
    }
    let grounded = hit[1] && dy < 0.0;

    // Fluids can support a step without reporting a solid landing.
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

/// [`step_horizontal`] that also slides against (and steps onto) dynamic
/// boxes.
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
    if f64::from(up) <= EPS {
        return normal;
    }
    let rise = f64::from(up);
    let rmin = [min[0], min[1] + rise, min[2]];
    let rmax = [max[0], max[1] + rise, max[2]];
    let (sx, sz) = slide_xz(rmin, rmax, dx, dz, &boxes_fn, dyn_boxes, ignore);
    // Keep the step only if the raised slide got us meaningfully further horizontally.
    if sx * sx + sz * sz <= nx * nx + nz * nz + 1e-9 {
        return normal;
    }
    // Settle back down onto the ledge (never below where we started).
    let smin = [rmin[0] + f64::from(sx), rmin[1], rmin[2] + f64::from(sz)];
    let smax = [rmax[0] + f64::from(sx), rmax[1], rmax[2] + f64::from(sz)];
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

/// The farthest a point may travel from `start` along unit `dir` (up to `max_dist`)
/// while keeping `pad` clearance from every collision box — the third-person camera
/// boom. Equivalent to sweeping a `2·pad` cube along the segment: each box is expanded
/// by `pad` (Minkowski) and the segment is slab-clipped against it; the nearest entry
/// distance wins. Returns `max_dist` when nothing blocks, and `0.0` when the start is
/// already inside an expanded box (the camera stays at the eye rather than clipping).
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
                let cell = [cx, cy, cz];
                for b in boxes_fn(cx, cy, cz) {
                    // Slab-clip the ray against the pad-expanded box.
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

/// Is point `p` inside any collision box of its cell? The particle test — a particle is a
/// point, not a body, so it stops the instant it enters a real box (a leg/top), passing
/// through the empty margin of an inset/model cell. EPS keeps a point exactly on a face
/// from counting, matching `sweep_axis`. Cell-local: boxes never extend past their cell
/// (model boxes are clipped per cell; a normal block's box *is* its cell).
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
