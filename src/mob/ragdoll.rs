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

use petramond_math::math::{voxel_at, IVec3, Vec3};

use super::model_meta::Skeleton;

const GRAVITY: f32 = -22.0;
const VEL_DAMP: f32 = 0.99;
const GROUND_FRICTION: f32 = 0.5;
const GROUND_PROBE: f32 = 0.1;
const ITERS: usize = 8;
const POLAR_ITERS: usize = 4;
const MAX_ANGULAR_SPEED: f32 = 24.0;
const MAX_JOINT_SWING: f32 = 0.8;
const LIFETIME: f32 = 1.8;
const POP_UP: f32 = 1.0;
const LAUNCH_SPEED: f32 = 2.5;
const LAUNCH_UP: f32 = 1.5;
/// How much the launch tumbles the corpse: the spin's edge velocity is this times the launch speed.
/// Kept below 1 so the launch always beats the spin and every corner moves away from the attacker,
/// never toward, while still giving a clear somersault. Scaling off the launch keeps that true
/// however `LAUNCH_SPEED` is tuned.
const SPIN_FRACTION: f32 = 0.25;
const CORNER_SPIN: f32 = 1.5;
const SEED_DT: f32 = 0.05;
const FACE_EPS: f32 = 1e-3;
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

pub struct Ragdoll {
    bones: Vec<RagBone>,
    age: f32,
    seed: u64,
    launch: Vec3,
    init: bool,
}

impl Ragdoll {
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

    #[inline]
    pub fn is_done(&self) -> bool {
        self.age >= LIFETIME
    }

    pub fn init(&mut self, skel: &Skeleton, scale: f32, mob_vel: Vec3, yaw: f32) {
        let to_model = Quat::from_rotation_y(-yaw);
        let launch = to_model * self.launch;
        let inherited = (to_model * mob_vel / scale) * 0.4;
        let launch_speed = LAUNCH_SPEED / scale;
        let launch_vel = launch * launch_speed;
        let up = POP_UP + LAUNCH_UP / scale;
        let centre = if skel.bones.is_empty() {
            Vec3::ZERO
        } else {
            skel.bones
                .iter()
                .map(|b| (b.bbox_min + b.bbox_max) * 0.5)
                .sum::<Vec3>()
                / skel.bones.len() as f32
        };
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

    /// Advances one tick. `mob_pos`, `yaw` and `scale` are the transform (frozen at death) that
    /// puts the model-space sim into the frame `solid(cell)` answers in. The caller picks that
    /// frame, which keeps a far-away corpse's sweeps small. `solid(cell)` says whether a block
    /// stops movement. Every corner collides against the real voxels, so the corpse can't sink
    /// through a floor or pass through a wall, and corners hanging over an edge keep falling.
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

        // The corpse's model-to-world transform, `global = T(pos)·Ry(yaw)·Scale`, and its inverse.
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
                escape_solid(wo, solid).unwrap_or(wo)
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
            // Shape-match each bone: fit a centroid and rotation to the corner cloud and snap the
            // corners back. So whatever moved the corners (gravity, collisions) ends up rotating
            // the bone.
            let max_rot = MAX_ANGULAR_SPEED * dt;
            for b in &mut self.bones {
                if b.weld.is_none() {
                    b.shape_match(max_rot);
                }
            }
            for b in &mut self.bones {
                if b.weld.is_some() {
                    continue;
                }
                for k in 0..8 {
                    b.nodes[k] = resolve(b.nodes_old[k], b.nodes[k]);
                }
            }
            for i in 0..self.bones.len() {
                if self.bones[i].weld.is_some() {
                    continue;
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

    #[cfg(test)]
    pub fn positions(&self) -> Vec<Vec3> {
        self.bones
            .iter()
            .map(|b| b.c + b.rot * (b.rest_pivot - b.c0))
            .collect()
    }

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

/// Moves corner `w`'s `axis` coordinate (world space) toward `target` one cell boundary at a time
/// and parks it just outside the first solid cell it hits. We walk instead of checking the end
/// point, because a fast move could otherwise skip a thin wall or clamp to a face buried in a
/// deeper solid cell. Returns the resolved coordinate.
fn sweep_axis(w: Vec3, axis: usize, target: f32, solid: &impl Fn(IVec3) -> bool) -> f32 {
    let start = w[axis];
    if !(start.is_finite() && target.is_finite()) {
        return target;
    }
    let target = target.clamp(start - MAX_SWEEP, start + MAX_SWEEP);
    let mut probe = w;
    if target > start {
        let mut face = start.floor() + 1.0;
        while face <= target {
            probe[axis] = face + FACE_EPS;
            if solid(voxel_at(probe)) {
                return face - FACE_EPS;
            }
            face += 1.0;
        }
    } else {
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

#[inline]
fn verlet(x: &mut Vec3, x_old: &mut Vec3, accel: Vec3, dt2: f32) {
    let vel = (*x - *x_old) * VEL_DAMP;
    let next = *x + vel + accel * dt2;
    *x_old = *x;
    *x = next;
}

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

#[inline]
fn outer(u: Vec3, v: Vec3) -> Mat3 {
    Mat3::from_cols(u * v.x, u * v.y, u * v.z)
}

/// Rotation (polar factor) of a cross-covariance matrix, via Müller et al.'s "A Robust Method to
/// Extract the Rotational Part of Deformations". Starting from `prev`, we rotate by the torque-like
/// residual until it lines up with the matrix. Bigger model boxes make a bigger matrix, but that
/// cancels in `omega`, unlike with a fixed-count Higham iteration. It never gives a reflection.
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
