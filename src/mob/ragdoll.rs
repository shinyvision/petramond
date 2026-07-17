//! A per-bone physics ragdoll for a dying mob — every bone is a full rigid body, not
//! just a hanging joint, so the corpse tumbles and falls over.
//!
//! Each bone is the box covering its geometry, simulated as its **8 corner particles**.
//! The corners fall under gravity and collide with the floor individually; a rigid
//! rotation + position is then recovered from the (now-deformed) corner cloud each tick
//! by *shape matching* (polar decomposition of the corner cross-covariance). Because the
//! corners hit the ground at different times, a bone that lands rotates — the body topples
//! onto its side instead of sinking flat. A light joint constraint then slides each bone
//! so its pivot meets the matching spot on its parent, keeping the skeleton connected
//! while each bone still tumbles on its own. Each joint also has a swing limit: a limb
//! sags under gravity relative to its parent only up to [`MAX_JOINT_SWING`], so legs
//! droop like dead weight but never fold through the body. Bones flagged
//! [`welded`](super::model_meta::SkBone::welded) (authored `_weld` name suffix, or a
//! cube-less animation-rig group) opt out of all of this: they ride their nearest
//! non-welded ancestor rigidly, so a hushjaw's teeth move with its jaw instead of
//! flapping on joints of their own, and its rig-only root never becomes a noise-driven
//! placeholder box that the joint pass would slave the real bones to.
//!
//! Everything is in the model's own units, so a sim-computed per-bone pose
//! `(pivot position, orientation)` drops straight into the render bake
//! (`global · pose[bone] · S_cube`). Gravity is the world value divided by the render
//! `scale` so the corpse falls at a real-world rate. The mob's `pos`/`yaw` (its `global`
//! transform) are frozen at death; the bones move in model space. Each corner is collided
//! against the real world voxels (converted through the frozen transform), so the corpse
//! can't pass through terrain and corners hanging over an edge keep falling.

use glam::{Mat3, Quat};

use crate::mathh::{voxel_at, IVec3, Vec3};

use super::model_meta::Skeleton;

/// Downward acceleration in WORLD units (m/s²); divided by the model scale per body.
const GRAVITY: f32 = -22.0;
/// Per-step velocity retention (Verlet damping) — mild air drag so motion bleeds off.
const VEL_DAMP: f32 = 0.99;
/// Horizontal velocity kept per floor contact — friction that skids a corpse to rest.
const GROUND_FRICTION: f32 = 0.5;
/// How far below a corner (WORLD m) to probe for a block when deciding it's "resting on
/// the ground" for friction.
const GROUND_PROBE: f32 = 0.1;
/// Constraint relaxation passes per tick (shape match + collision + joints).
const ITERS: usize = 8;
/// Polar-decomposition iterations to extract a rotation from the corner cloud.
const POLAR_ITERS: usize = 4;
/// Maximum recovered angular speed for one ragdoll body. Shape matching can otherwise
/// turn a noisy corner contact into an implausible full-body spin, especially for compact
/// models with many constrained child bodies.
const MAX_ANGULAR_SPEED: f32 = 24.0;
/// Maximum rotation (radians) a child bone may deviate from its rest orientation
/// *relative to its parent* — a joint swing limit. Gravity still drags a limb down (its
/// corners fall like everything else), but it sags onto this limit instead of folding
/// through the body.
const MAX_JOINT_SWING: f32 = 0.8;
/// Seconds the corpse ragdolls before it despawns.
const LIFETIME: f32 = 1.8;
/// Upward pop (model-units/s) given on death — a small base lurch before the collapse.
const POP_UP: f32 = 1.0;
/// Horizontal speed (WORLD m/s) the killing blow launches the corpse in the punched
/// direction. Converted to model units per body (÷ scale).
const LAUNCH_SPEED: f32 = 2.5;
/// Upward speed (WORLD m/s) added to the launch so the corpse flies in an arc.
const LAUNCH_UP: f32 = 1.5;
/// How much the launch tumbles the corpse, as a fraction of the launch speed: the spin's
/// edge velocity is this × the launch speed. Kept below 1 so the launch (translation)
/// always dominates the spin — every corner's net motion is *away*, never toward the
/// attacker — while still giving a clear somersault. Scaling the spin to the launch this
/// way keeps that guarantee no matter how `LAUNCH_SPEED` is tuned.
const SPIN_FRACTION: f32 = 0.25;
/// Per-corner velocity spread (model-units/s) seeded on death, so bones don't move
/// perfectly rigidly — a little natural variation on top of the coherent tumble.
const CORNER_SPIN: f32 = 1.5;
/// The tick length assumed when seeding initial Verlet velocities (20 TPS).
const SEED_DT: f32 = 0.05;
/// How far outside a block face a clamped corner is parked, so it doesn't re-classify as
/// inside the solid cell next test.
const FACE_EPS: f32 = 1e-3;
/// Longest per-axis move (WORLD m) a corner sweep will walk in one resolve. Real corpse
/// motion tops out around 2 m/tick; the cap keeps a corrupted position from turning the
/// boundary walk into a spin.
const MAX_SWEEP: f32 = 16.0;
const EPS: f32 = 1e-5;

/// One bone as a rigid body: its 8 box-corner Verlet particles, plus the rest geometry
/// (corner offsets from the rest centroid, the rest centroid, and the pivot) needed to
/// shape-match a rotation and to attach to its parent. `c`/`rot` are the recovered
/// centroid + orientation; the `prev_*` fields snapshot the tick start for interpolation.
///
/// A bone with `weld: Some(anchor)` runs no physics of its own — no integration, no
/// shape matching, no collision, no joint: its `c`/`rot` are derived rigidly from the
/// anchor (its nearest non-welded ancestor) each iteration, and its `nodes` are unused
/// after init.
struct RagBone {
    nodes: [Vec3; 8],
    nodes_old: [Vec3; 8],
    rest: [Vec3; 8],
    c0: Vec3,
    rest_pivot: Vec3,
    parent: Option<usize>,
    weld: Option<usize>,
    c: Vec3,
    rot: Quat,
    prev_c: Vec3,
    prev_rot: Quat,
}

/// A live ragdoll: one rigid-body bone per skeleton bone plus an age timer. Constructed
/// [`pending`](Ragdoll::pending) the instant a mob dies, [`init`](Ragdoll::init)ialised
/// on the next tick, then [`step`](Ragdoll::step)ped each tick (collided against the
/// world's blocks).
pub struct Ragdoll {
    bones: Vec<RagBone>,
    age: f32,
    seed: u64,
    /// World-space horizontal unit direction the killing blow flung the corpse (away
    /// from the attacker), or `ZERO` if there was no clear direction. Drives the launch
    /// + somersault applied at [`init`](Self::init).
    launch: Vec3,
    init: bool,
}

impl Ragdoll {
    /// A ragdoll awaiting initialisation (deferred to the first dead tick). `seed` drives
    /// the per-bone fling; `launch` is the (world-space, horizontal) direction the killing
    /// blow flung the corpse.
    pub fn pending(seed: u64, launch: Vec3) -> Self {
        Ragdoll {
            bones: Vec::new(),
            age: 0.0,
            seed,
            launch,
            init: false,
        }
    }

    #[inline]
    pub fn is_initialized(&self) -> bool {
        self.init
    }

    /// The corpse has flopped long enough and should be removed from the world.
    #[inline]
    pub fn is_done(&self) -> bool {
        self.age >= LIFETIME
    }

    /// Seed each bone's corners from the rest skeleton plus the killing blow's launch: a
    /// directional fling in the punched direction with an arc and a coherent somersault,
    /// so the whole corpse is sent flying and tumbling. `mob_vel` is the mob's velocity
    /// at death (world units), `scale` the model→metre scale, `yaw` the mob's facing at
    /// death.
    pub fn init(&mut self, skel: &Skeleton, scale: f32, mob_vel: Vec3, yaw: f32) {
        // The sim runs in the model's LOCAL space, but the renderer re-applies the mob's
        // yaw (`global = T(pos)·Ry(yaw)·Scale`). So world-space directions — the launch
        // and the carried velocity — must be un-rotated by the yaw into model space here,
        // or the corpse would fly off in a yaw-dependent (effectively random) direction.
        let to_model = Quat::from_rotation_y(-yaw);
        let launch = to_model * self.launch;
        // A little carried momentum (model units), softened.
        let inherited = (to_model * mob_vel / scale) * 0.4;
        // The killing blow's launch (punched direction) + an upward arc, in model units.
        let launch_speed = LAUNCH_SPEED / scale;
        let launch_vel = launch * launch_speed;
        let up = POP_UP + LAUNCH_UP / scale;
        // The mob's overall rest centre — the pivot the whole-body somersault turns about,
        // so every bone's corners share one coherent rotation (not per-bone spins).
        let centre = if skel.bones.is_empty() {
            Vec3::ZERO
        } else {
            skel.bones
                .iter()
                .map(|b| (b.bbox_min + b.bbox_max) * 0.5)
                .sum::<Vec3>()
                / skel.bones.len() as f32
        };
        // Angular velocity about a horizontal axis across the (model-space) launch → a
        // forward somersault in the flight direction. Its magnitude is scaled so the
        // spin's edge velocity is `SPIN_FRACTION` of the launch speed — so the launch
        // always out-runs the spin and the corpse never swings toward the attacker. Zero
        // if there's no launch direction.
        let radius = skel
            .bones
            .iter()
            .flat_map(|b| corners(b.bbox_min, b.bbox_max))
            .map(|c| (c - centre).length())
            .fold(0.0_f32, f32::max)
            .max(EPS);
        let omega = if launch.length_squared() > EPS {
            Vec3::Y.cross(launch).normalize_or_zero() * (SPIN_FRACTION * launch_speed / radius)
        } else {
            Vec3::ZERO
        };
        // A welded bone rides its nearest non-welded ancestor (its anchor) instead of
        // simulating a rigid body of its own — teeth move with the jaw, never on their
        // own joints. Welded chains resolve transitively; a broken chain (no reachable
        // physical ancestor) falls back to simulating the bone normally.
        let anchor_of = |bone: usize| -> Option<usize> {
            if !skel.bones[bone].welded {
                return None;
            }
            let mut next = skel.bones[bone].parent;
            for _ in 0..skel.bones.len() {
                let p = next?;
                if !skel.bones[p].welded {
                    return Some(p);
                }
                next = skel.bones[p].parent;
            }
            None
        };
        self.bones = skel
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let h = |salt: u64| {
                    crate::entity::hash01(
                        self.seed
                            ^ (i as u64)
                                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                                .wrapping_add(salt),
                    )
                };
                let cs = corners(b.bbox_min, b.bbox_max);
                let c0 = (b.bbox_min + b.bbox_max) * 0.5;
                // Shared per-bone velocity: carried momentum + directional launch + a
                // small splay + the upward arc.
                let base = inherited
                    + launch_vel
                    + Vec3::new((h(1) - 0.5) * 1.5, up + h(2) * 0.5, (h(3) - 0.5) * 1.5);
                let mut nodes = [Vec3::ZERO; 8];
                let mut nodes_old = [Vec3::ZERO; 8];
                let mut rest = [Vec3::ZERO; 8];
                for (k, corner) in cs.into_iter().enumerate() {
                    let jitter = Vec3::new(
                        h(10 + k as u64) - 0.5,
                        h(20 + k as u64) - 0.5,
                        h(30 + k as u64) - 0.5,
                    ) * CORNER_SPIN;
                    // `omega × (corner − centre)` is the rigid somersault velocity of this
                    // corner; shared across all bones it tumbles the whole corpse as one.
                    let v = base + omega.cross(corner - centre) + jitter;
                    nodes[k] = corner;
                    nodes_old[k] = corner - v * SEED_DT;
                    rest[k] = corner - c0;
                }
                RagBone {
                    nodes,
                    nodes_old,
                    rest,
                    c0,
                    rest_pivot: b.pivot,
                    parent: b.parent,
                    weld: anchor_of(i),
                    c: c0,
                    rot: Quat::IDENTITY,
                    prev_c: c0,
                    prev_rot: Quat::IDENTITY,
                }
            })
            .collect();
        self.init = true;
    }

    /// Advance one tick. `mob_pos`/`yaw`/`scale` are the (frozen-at-death) transform that
    /// places the model-space sim into the world; `solid(cell)` reports whether a world
    /// block stops movement. Each corner is collided against the real voxels — so the
    /// corpse can't sink through a floor or pass through a wall, and corners hanging over
    /// an edge keep falling.
    pub fn step(
        &mut self,
        dt: f32,
        scale: f32,
        mob_pos: Vec3,
        yaw: f32,
        solid: &impl Fn(IVec3) -> bool,
    ) {
        for b in &mut self.bones {
            b.prev_c = b.c;
            b.prev_rot = b.rot;
        }

        // Model↔world transforms for this corpse (its `global = T(pos)·Ry(yaw)·Scale`).
        let ry = Quat::from_rotation_y(yaw);
        let ry_inv = Quat::from_rotation_y(-yaw);
        let world_of = |mp: Vec3| mob_pos + ry * (mp * scale);
        // Per-axis voxel resolve: sweep model-space `cur` from collision-free `old`,
        // walking every cell boundary the move crosses on each axis and parking just
        // outside the face of the first solid cell entered. Endpoint-only tests are not
        // enough: a corpse falls up to ~2 m per tick by the end of its lifetime, which
        // skips a one-cell floor entirely. Axis order X, Z, Y so landing is decided
        // last. A corner that STARTS inside a solid cell (the joint pass runs after the
        // last collision pass and can slide one in; a mob can die with geometry inside
        // a movement-blocking cell, e.g. standing on a partial block) is healed out of
        // the nearest open face — never resolved with collision disabled, which would
        // let the corner (and, through shape matching, its whole limb) fall through the
        // world. Returns the resolved model position.
        let resolve = |old: Vec3, cur: Vec3| -> Vec3 {
            let wo = world_of(old);
            let w = if solid(voxel_at(wo)) {
                escape_solid(wo, solid).unwrap_or(wo) // sealed on all sides: hold still
            } else {
                let mut w = wo;
                let wc = world_of(cur);
                w.x = sweep_axis(w, 0, wc.x, solid);
                w.z = sweep_axis(w, 2, wc.z, solid);
                w.y = sweep_axis(w, 1, wc.y, solid);
                w
            };
            ry_inv * (w - mob_pos) / scale
        };

        // Integrate every corner, then bleed horizontal speed on any corner resting on a
        // block (ground friction, applied once per tick via the Verlet previous position).
        let accel = Vec3::new(0.0, GRAVITY / scale, 0.0);
        let dt2 = dt * dt;
        let probe = Vec3::new(0.0, GROUND_PROBE / scale, 0.0);
        for b in &mut self.bones {
            if b.weld.is_some() {
                continue;
            }
            for k in 0..8 {
                verlet(&mut b.nodes[k], &mut b.nodes_old[k], accel, dt2);
                if solid(voxel_at(world_of(b.nodes[k] - probe))) {
                    let v = b.nodes[k] - b.nodes_old[k];
                    b.nodes_old[k].x = b.nodes[k].x - v.x * GROUND_FRICTION;
                    b.nodes_old[k].z = b.nodes[k].z - v.z * GROUND_FRICTION;
                }
            }
        }

        for _ in 0..ITERS {
            // Shape-match each bone: recover its rigid centroid + rotation from the corner
            // cloud, then snap the corners back onto that rigid shape. The corners' motion
            // (gravity + collisions) becomes the bone's rotation.
            let max_rot = MAX_ANGULAR_SPEED * dt;
            for b in &mut self.bones {
                if b.weld.is_none() {
                    b.shape_match(max_rot);
                }
            }
            // Collision: resolve every corner against the voxels (from its start-of-tick
            // position, which is collision-free), so the rigid shape doesn't sink into a
            // block.
            for b in &mut self.bones {
                if b.weld.is_some() {
                    continue;
                }
                for k in 0..8 {
                    b.nodes[k] = resolve(b.nodes_old[k], b.nodes[k]);
                }
            }
            // Joints (last, so the connection stays exact at render): clamp each child's
            // rotation to a bounded swing about its rest orientation relative to its
            // parent (legs sag under gravity but can't fold into the body), then slide it
            // so its pivot meets the spot on its parent it attaches to (root-first order).
            for i in 0..self.bones.len() {
                if self.bones[i].weld.is_some() {
                    continue; // the anchor drives it; a joint would fight the weld
                }
                let Some(p) = self.bones[i].parent else {
                    continue;
                };
                let (pc, pr, pc0) = (self.bones[p].c, self.bones[p].rot, self.bones[p].c0);
                let rel = (pr.inverse() * self.bones[i].rot).normalize();
                let lim = clamp_rotation(Quat::IDENTITY, rel, MAX_JOINT_SWING);
                if lim.angle_between(rel) > EPS {
                    let b = &mut self.bones[i];
                    b.rot = (pr * lim).normalize();
                    for k in 0..8 {
                        b.nodes[k] = b.c + b.rot * b.rest[k];
                    }
                }
                let rp = self.bones[i].rest_pivot;
                let target = pc + pr * (rp - pc0);
                let cur = self.bones[i].c + self.bones[i].rot * (rp - self.bones[i].c0);
                let shift = target - cur;
                let b = &mut self.bones[i];
                for k in 0..8 {
                    b.nodes[k] += shift;
                }
                b.c += shift;
            }
            // Welded bones ride their anchor rigidly. Synced after the joint pass so
            // they match the anchor's post-constraint pose exactly (and a physical bone
            // jointed to a welded parent reads coherent parent state next iteration).
            for i in 0..self.bones.len() {
                let Some(a) = self.bones[i].weld else {
                    continue;
                };
                let (ac, ar, ac0) = (self.bones[a].c, self.bones[a].rot, self.bones[a].c0);
                let b = &mut self.bones[i];
                b.rot = ar;
                b.c = ac + ar * (b.c0 - ac0);
            }
        }
        self.age += dt;
    }

    /// Interpolated per-bone `(pivot position, orientation)` at `alpha` into the tick, for
    /// the render bake to turn into `T(pos)·R(rot)·T(-pivot)` poses (the pivot position is
    /// the rigid transform applied to the bone's rest pivot). Empty until
    /// [`init`](Self::init) runs (the renderer then falls back to the rest pose).
    pub fn pose(&self, alpha: f32) -> Vec<(Vec3, Quat)> {
        let interp: Vec<(Vec3, Quat)> = self
            .bones
            .iter()
            .map(|b| (b.prev_c.lerp(b.c, alpha), b.prev_rot.slerp(b.rot, alpha)))
            .collect();
        self.bones
            .iter()
            .enumerate()
            .map(|(i, b)| {
                // A welded bone is posed by its anchor's INTERPOLATED transform — its own
                // lerped centroid would cut the chord of the anchor's rotation arc and
                // let it drift off the anchor mid-tick.
                let (c, rot, c0) = match b.weld {
                    Some(a) => (interp[a].0, interp[a].1, self.bones[a].c0),
                    None => (interp[i].0, interp[i].1, b.c0),
                };
                (c + rot * (b.rest_pivot - c0), rot)
            })
            .collect()
    }

    /// Current bone pivot (joint) positions — for tests asserting connectivity.
    #[cfg(test)]
    pub fn positions(&self) -> Vec<Vec3> {
        self.bones
            .iter()
            .map(|b| b.c + b.rot * (b.rest_pivot - b.c0))
            .collect()
    }

    /// The lowest corner (model-space y) across all simulated bones — for collision
    /// tests. Welded bones are skipped: their nodes are unused after init and frozen at
    /// the rest pose.
    #[cfg(test)]
    pub fn lowest_node_y(&self) -> f32 {
        self.bones
            .iter()
            .filter(|b| b.weld.is_none())
            .flat_map(|b| b.nodes)
            .map(|n| n.y)
            .fold(f32::INFINITY, f32::min)
    }
}

impl RagBone {
    /// Recover this bone's rigid centroid + rotation from its corner cloud (shape
    /// matching), then snap the corners back onto the rigid shape. The recovered rotation
    /// is the bone's physical orientation; snapping keeps it rigid while the corners'
    /// gravity/ground motion drives that rotation.
    fn shape_match(&mut self, max_rot: f32) {
        let c = self.nodes.iter().copied().sum::<Vec3>() / 8.0;
        let mut a = Mat3::ZERO;
        for k in 0..8 {
            a += outer(self.nodes[k] - c, self.rest[k]);
        }
        let rot = clamp_rotation(self.prev_rot, extract_rotation(a, self.rot), max_rot);
        self.c = c;
        self.rot = rot;
        for k in 0..8 {
            self.nodes[k] = c + rot * self.rest[k];
        }
    }
}

/// Walk coordinate `axis` of corner `w` (WORLD space) toward `target`, one cell boundary
/// at a time, parking just outside the face of the first solid cell entered. Unlike
/// testing only the endpoint, the walk can neither skip over a thin obstacle nor clamp
/// to a face that lies inside a deeper solid cell when the move crosses several cells in
/// one tick. Returns the resolved coordinate.
fn sweep_axis(w: Vec3, axis: usize, target: f32, solid: &impl Fn(IVec3) -> bool) -> f32 {
    let start = w[axis];
    if !(start.is_finite() && target.is_finite()) {
        return target;
    }
    let target = target.clamp(start - MAX_SWEEP, start + MAX_SWEEP);
    let mut probe = w;
    if target > start {
        // Entering the cell ABOVE each face crossed (`face` is also that cell's index).
        let mut face = start.floor() + 1.0;
        while face <= target {
            probe[axis] = face + FACE_EPS;
            if solid(voxel_at(probe)) {
                return face - FACE_EPS;
            }
            face += 1.0;
        }
    } else {
        // Entering the cell BELOW each face crossed. A coordinate exactly on a boundary
        // classifies into the cell above (`voxel_at` floors), so crossing is strict.
        let mut face = start.floor();
        while target < face {
            probe[axis] = face - FACE_EPS;
            if solid(voxel_at(probe)) {
                return face + FACE_EPS;
            }
            face -= 1.0;
        }
    }
    target
}

/// The corner sits inside a solid cell: push it just outside the nearest cell face whose
/// neighbouring cell is open. The pop is minimal — an embedded corner is barely past a
/// face — so healing is invisible. `None` when all six neighbours are solid (sealed in;
/// the caller holds the corner in place rather than dropping it through the world).
fn escape_solid(w: Vec3, solid: &impl Fn(IVec3) -> bool) -> Option<Vec3> {
    let cell = voxel_at(w);
    let lo = cell.as_vec3();
    let mut best: Option<(f32, Vec3)> = None;
    for axis in 0..3 {
        let exits = [
            (w[axis] - lo[axis], lo[axis] - FACE_EPS, -1),
            (lo[axis] + 1.0 - w[axis], lo[axis] + 1.0 + FACE_EPS, 1),
        ];
        for (dist, coord, step) in exits {
            let mut neighbour = cell;
            neighbour[axis] += step;
            if solid(neighbour) || best.is_some_and(|(d, _)| d <= dist) {
                continue;
            }
            let mut out = w;
            out[axis] = coord;
            best = Some((dist, out));
        }
    }
    best.map(|(_, out)| out)
}

/// One Verlet integration step for a particle: `x += (x - x_old)·damp + accel·dt²`,
/// rolling `x_old` to the pre-step position.
#[inline]
fn verlet(x: &mut Vec3, x_old: &mut Vec3, accel: Vec3, dt2: f32) {
    let vel = (*x - *x_old) * VEL_DAMP;
    let next = *x + vel + accel * dt2;
    *x_old = *x;
    *x = next;
}

/// The eight corners of the box `[min, max]`.
fn corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, max.y, max.z),
    ]
}

/// Outer product `u ⊗ v` as a 3×3 matrix (column `k` is `u · v[k]`).
#[inline]
fn outer(u: Vec3, v: Vec3) -> Mat3 {
    Mat3::from_cols(u * v.x, u * v.y, u * v.z)
}

/// Extract the rotation (polar factor) from a cross-covariance matrix by warm-started
/// incremental extraction (Müller et al., "A Robust Method to Extract the Rotational Part
/// of Deformations"): rotate `prev` by the torque-like residual until it aligns with the
/// matrix. Unlike a fixed-count Higham iteration this is scale-invariant — the matrix's
/// magnitude (which grows with the model's box sizes) cancels in the `omega` quotient —
/// and it always yields a proper rotation, never a reflection.
fn extract_rotation(a: Mat3, prev: Quat) -> Quat {
    if !(a.x_axis.is_finite() && a.y_axis.is_finite() && a.z_axis.is_finite()) {
        return prev;
    }
    let mut q = prev;
    for _ in 0..POLAR_ITERS {
        let r = Mat3::from_quat(q);
        let numer = r.x_axis.cross(a.x_axis) + r.y_axis.cross(a.y_axis) + r.z_axis.cross(a.z_axis);
        let denom = r.x_axis.dot(a.x_axis) + r.y_axis.dot(a.y_axis) + r.z_axis.dot(a.z_axis);
        let omega = numer / (denom.abs() + EPS);
        let angle = omega.length();
        if !angle.is_finite() || angle < 1e-7 {
            break;
        }
        q = (Quat::from_axis_angle(omega / angle, angle) * q).normalize();
    }
    q
}

fn clamp_rotation(from: Quat, to: Quat, max_angle: f32) -> Quat {
    let angle = from.angle_between(to);
    if !angle.is_finite() || angle <= max_angle.max(0.0) {
        return to;
    }
    from.slerp(to, max_angle / angle).normalize()
}

#[cfg(test)]
mod tests;
