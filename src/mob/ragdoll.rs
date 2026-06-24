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
//! while each bone still tumbles on its own.
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
const SPIN_FRACTION: f32 = 0.5;
/// Per-corner velocity spread (model-units/s) seeded on death, so bones don't move
/// perfectly rigidly — a little natural variation on top of the coherent tumble.
const CORNER_SPIN: f32 = 1.5;
/// The tick length assumed when seeding initial Verlet velocities (20 TPS).
const SEED_DT: f32 = 0.05;
/// How far outside a block face a clamped corner is parked, so it doesn't re-classify as
/// inside the solid cell next test.
const FACE_EPS: f32 = 1e-3;
const EPS: f32 = 1e-5;

/// One bone as a rigid body: its 8 box-corner Verlet particles, plus the rest geometry
/// (corner offsets from the rest centroid, the rest centroid, and the pivot) needed to
/// shape-match a rotation and to attach to its parent. `c`/`rot` are the recovered
/// centroid + orientation; the `prev_*` fields snapshot the tick start for interpolation.
struct RagBone {
    nodes: [Vec3; 8],
    nodes_old: [Vec3; 8],
    rest: [Vec3; 8],
    c0: Vec3,
    rest_pivot: Vec3,
    parent: Option<usize>,
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
        // Per-axis voxel resolve: sweep model-space `cur` from collision-free `old`, and
        // for any axis whose move would enter a solid cell, clamp it to that cell's face
        // (so a corner rests *on* the surface instead of hovering a step above it). Axis
        // order X, Z, Y so landing is decided last. Returns the resolved model position.
        let resolve = |old: Vec3, cur: Vec3| -> Vec3 {
            let wo = world_of(old);
            if solid(voxel_at(wo)) {
                return cur; // started inside a block (shouldn't happen): don't trap it
            }
            let wc = world_of(cur);
            let mut w = wo;
            // Clamp `axis` of `w` to the face of the cell it would enter, given the move
            // direction (entered the low face moving +, the high face moving -).
            let face = |coord: f32, moving_pos: bool| {
                if moving_pos {
                    coord.floor() - FACE_EPS
                } else {
                    coord.floor() + 1.0 + FACE_EPS
                }
            };
            w.x = wc.x;
            if solid(voxel_at(w)) {
                w.x = face(wc.x, wc.x > wo.x);
            }
            w.z = wc.z;
            if solid(voxel_at(w)) {
                w.z = face(wc.z, wc.z > wo.z);
            }
            w.y = wc.y;
            if solid(voxel_at(w)) {
                w.y = face(wc.y, wc.y > wo.y);
            }
            ry_inv * (w - mob_pos) / scale
        };

        // Integrate every corner, then bleed horizontal speed on any corner resting on a
        // block (ground friction, applied once per tick via the Verlet previous position).
        let accel = Vec3::new(0.0, GRAVITY / scale, 0.0);
        let dt2 = dt * dt;
        let probe = Vec3::new(0.0, GROUND_PROBE / scale, 0.0);
        for b in &mut self.bones {
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
            for b in &mut self.bones {
                b.shape_match();
            }
            // Collision: resolve every corner against the voxels (from its start-of-tick
            // position, which is collision-free), so the rigid shape doesn't sink into a
            // block.
            for b in &mut self.bones {
                for k in 0..8 {
                    b.nodes[k] = resolve(b.nodes_old[k], b.nodes[k]);
                }
            }
            // Joints (last, so the connection stays exact at render): slide each child so
            // its pivot meets the spot on its parent it attaches to (root-first order).
            for i in 0..self.bones.len() {
                let Some(p) = self.bones[i].parent else {
                    continue;
                };
                let (pc, pr, pc0) = (self.bones[p].c, self.bones[p].rot, self.bones[p].c0);
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
        }
        self.age += dt;
    }

    /// Interpolated per-bone `(pivot position, orientation)` at `alpha` into the tick, for
    /// the render bake to turn into `T(pos)·R(rot)·T(-pivot)` poses (the pivot position is
    /// the rigid transform applied to the bone's rest pivot). Empty until
    /// [`init`](Self::init) runs (the renderer then falls back to the rest pose).
    pub fn pose(&self, alpha: f32) -> Vec<(Vec3, Quat)> {
        self.bones
            .iter()
            .map(|b| {
                let c = b.prev_c.lerp(b.c, alpha);
                let rot = b.prev_rot.slerp(b.rot, alpha);
                (c + rot * (b.rest_pivot - b.c0), rot)
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

    /// The lowest corner (model-space y) across all bones — for collision tests.
    #[cfg(test)]
    pub fn lowest_node_y(&self) -> f32 {
        self.bones
            .iter()
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
    fn shape_match(&mut self) {
        let c = self.nodes.iter().copied().sum::<Vec3>() / 8.0;
        let mut a = Mat3::ZERO;
        for k in 0..8 {
            a += outer(self.nodes[k] - c, self.rest[k]);
        }
        let rot = extract_rotation(a, self.rot);
        self.c = c;
        self.rot = rot;
        for k in 0..8 {
            self.nodes[k] = c + rot * self.rest[k];
        }
    }
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

/// Extract the rotation from a cross-covariance matrix by polar decomposition
/// (iterative averaging with the inverse-transpose). Falls back to `prev` if the matrix
/// is degenerate or the result isn't a proper rotation.
fn extract_rotation(a: Mat3, prev: Quat) -> Quat {
    let det = a.determinant();
    if !det.is_finite() || det.abs() < EPS {
        return prev;
    }
    let mut q = a;
    for _ in 0..POLAR_ITERS {
        let inv_t = q.inverse().transpose();
        q = (q + inv_t) * 0.5;
    }
    if !q.determinant().is_finite() || q.determinant() <= 0.0 {
        return prev;
    }
    Quat::from_mat3(&q).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::model_meta::SkBone;

    fn boxed(pivot: Vec3, min: Vec3, max: Vec3, parent: Option<usize>) -> SkBone {
        SkBone {
            pivot,
            bbox_min: min,
            bbox_max: max,
            parent,
        }
    }

    /// A solid floor (every cell below world y = 0) for the ragdoll tests.
    fn floor(c: IVec3) -> bool {
        c.y < 0
    }

    /// A root box with one child box stacked above it, jointed at their shared face.
    fn two_bone_skeleton() -> Skeleton {
        Skeleton {
            bones: vec![
                boxed(
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(-0.5, 0.5, -0.5),
                    Vec3::new(0.5, 1.5, 0.5),
                    None,
                ),
                boxed(
                    Vec3::new(0.0, 1.5, 0.0),
                    Vec3::new(-0.5, 1.5, -0.5),
                    Vec3::new(0.5, 2.5, 0.5),
                    Some(0),
                ),
            ],
        }
    }

    #[test]
    fn ragdoll_stays_connected_settles_above_ground_and_finishes() {
        let skel = two_bone_skeleton();
        let joint_rest = (skel.bones[1].pivot - skel.bones[0].pivot).length();
        // Launch + tumble — the realistic case — must NOT pull the joint apart.
        let mut rag = Ragdoll::pending(42, Vec3::X);
        rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
        assert!(rag.is_initialized());

        for _ in 0..(LIFETIME / 0.05) as usize + 1 {
            rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
            let p = rag.positions();
            // The joint pass keeps the child's joint locked to the root's; a rigid
            // rotation preserves the pivot-to-pivot distance, so this stays tight even as
            // the corpse flies and somersaults.
            let d = (p[1] - p[0]).length();
            assert!(
                (d - joint_rest).abs() < 0.1,
                "joints stay connected: {d} vs {joint_rest}"
            );
        }
        assert!(rag.is_done(), "the ragdoll finishes after its lifetime");
    }

    #[test]
    fn the_body_tumbles_and_falls_over() {
        // A tall box dropped onto the floor must rotate (tip/tumble) — the whole rigid
        // body is simulated, not just joints.
        let skel = Skeleton {
            bones: vec![boxed(
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(-0.5, 3.0, -0.5),
                Vec3::new(0.5, 6.0, 0.5),
                None,
            )],
        };
        let mut rag = Ragdoll::pending(7, Vec3::X);
        rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
        let mut max_angle = 0.0f32;
        for _ in 0..36 {
            rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
            let rot = rag.pose(1.0)[0].1;
            max_angle = max_angle.max(rot.angle_between(Quat::IDENTITY));
        }
        assert!(
            max_angle > 0.2,
            "the body rotated under physics (tipped/tumbled): {max_angle}"
        );
    }

    #[test]
    fn the_killing_blow_flings_the_corpse_in_the_punched_direction() {
        // A box flung toward +X (high up, so it stays airborne) should travel +X.
        let skel = Skeleton {
            bones: vec![boxed(
                Vec3::new(0.0, 10.0, 0.0),
                Vec3::new(-0.5, 10.0, -0.5),
                Vec3::new(0.5, 11.0, 0.5),
                None,
            )],
        };
        let mut rag = Ragdoll::pending(3, Vec3::X);
        rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
        let x0 = rag.pose(1.0)[0].0.x;
        for _ in 0..8 {
            rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
        }
        let x1 = rag.pose(1.0)[0].0.x;
        assert!(
            x1 > x0 + 1.0,
            "the corpse flies in the punched (+X) direction: {x0} -> {x1}"
        );
    }

    #[test]
    fn the_launch_never_drags_a_bone_toward_the_attacker() {
        // Two boxes stacked vertically (the lower one below the mob centre), flung +X high
        // up. The spin is bounded below the launch, so EVERY bone must travel +X (away) —
        // none swings back toward the attacker (the bug this guards).
        let skel = Skeleton {
            bones: vec![
                boxed(
                    Vec3::new(0.0, 11.0, 0.0),
                    Vec3::new(-0.5, 11.0, -0.5),
                    Vec3::new(0.5, 12.0, 0.5),
                    None,
                ),
                boxed(
                    Vec3::new(0.0, 10.0, 0.0),
                    Vec3::new(-0.5, 9.0, -0.5),
                    Vec3::new(0.5, 10.0, 0.5),
                    Some(0),
                ),
            ],
        };
        let mut rag = Ragdoll::pending(5, Vec3::X);
        rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
        let x0: Vec<f32> = rag.pose(1.0).iter().map(|p| p.0.x).collect();
        for _ in 0..6 {
            rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
        }
        let x1: Vec<f32> = rag.pose(1.0).iter().map(|p| p.0.x).collect();
        for (a, b) in x0.iter().zip(&x1) {
            assert!(
                b > a,
                "every bone flies away (+X), none toward the attacker: {a} -> {b}"
            );
        }
    }

    #[test]
    fn the_launch_is_world_space_regardless_of_facing() {
        // The corpse must fly in the WORLD launch direction even when the mob faced some
        // other way at death — the renderer re-applies the mob's yaw, so the sim stores
        // the launch un-rotated into model space. (Without this, flight direction depends
        // on facing and looks random.)
        let skel = Skeleton {
            bones: vec![boxed(
                Vec3::new(0.0, 10.0, 0.0),
                Vec3::new(-0.5, 10.0, -0.5),
                Vec3::new(0.5, 11.0, 0.5),
                None,
            )],
        };
        let yaw = 1.3; // a non-zero facing
        let mut rag = Ragdoll::pending(3, Vec3::X); // world launch = +X
        rag.init(&skel, 0.25, Vec3::ZERO, yaw);
        let p0 = rag.pose(1.0)[0].0;
        for _ in 0..8 {
            rag.step(0.05, 0.25, Vec3::ZERO, yaw, &floor);
        }
        let p1 = rag.pose(1.0)[0].0;
        // The render applies `Ry(yaw)` to the model-space position, so transform the
        // displacement the same way and check it points along world +X.
        let disp = glam::Quat::from_rotation_y(yaw) * (p1 - p0);
        assert!(disp.x > 1.0, "flies along world +X: {disp:?}");
        assert!(
            disp.z.abs() < disp.x,
            "mostly +X, not flung sideways: {disp:?}"
        );
    }

    #[test]
    fn a_corpse_rests_on_a_block_and_does_not_sink_through() {
        // A box dropped onto a solid floor (cells below world y=0) must settle on top, not
        // pass through it. scale 1.0 → model space == world space.
        let skel = Skeleton {
            bones: vec![boxed(
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(-0.5, 3.0, -0.5),
                Vec3::new(0.5, 4.0, 0.5),
                None,
            )],
        };
        let mut rag = Ragdoll::pending(2, Vec3::ZERO); // no launch: drops straight down
        rag.init(&skel, 1.0, Vec3::ZERO, 0.0);
        for _ in 0..80 {
            rag.step(0.05, 1.0, Vec3::ZERO, 0.0, &floor);
        }
        assert!(
            rag.lowest_node_y() > -0.2,
            "corpse rests on the block top, doesn't sink through: {}",
            rag.lowest_node_y()
        );
    }

    #[test]
    fn a_corpse_falls_off_the_edge_of_a_block() {
        // The floor only covers x < 0. A box dropped straddling the edge must drape/fall
        // off the unsupported (+X) side — its lowest corner ends well below the floor top.
        let solid = |c: IVec3| c.y < 0 && c.x < 0;
        let skel = Skeleton {
            bones: vec![boxed(
                Vec3::new(0.0, 4.0, 0.0),
                Vec3::new(-0.5, 4.0, -0.5),
                Vec3::new(0.5, 5.0, 0.5),
                None,
            )],
        };
        let mut rag = Ragdoll::pending(9, Vec3::ZERO);
        rag.init(&skel, 1.0, Vec3::ZERO, 0.0);
        for _ in 0..80 {
            rag.step(0.05, 1.0, Vec3::ZERO, 0.0, &solid);
        }
        assert!(
            rag.lowest_node_y() < -0.5,
            "corpse drops off the unsupported edge: lowest {}",
            rag.lowest_node_y()
        );
    }

    #[test]
    fn uninitialised_ragdoll_has_no_pose() {
        let rag = Ragdoll::pending(1, Vec3::ZERO);
        assert!(!rag.is_initialized());
        assert!(rag.pose(0.5).is_empty(), "no bones before init");
    }
}
