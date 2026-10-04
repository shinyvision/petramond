//! A per-bone physics ragdoll for a dying mob — every bone is a full rigid body, not
//! just a hanging joint, so the corpse tumbles and falls over.
//!
//! Each bone is the box covering its geometry, simulated as its **8 corner particles**.
//! The corners fall under gravity and collide with the floor individually; a rigid
//! rotation + position is then recovered from the (now-deformed) corner cloud each tick
//! by *shape matching* (polar decomposition of the corner cross-covariance). Because the
//! corners hit the ground at different times, a bone that lands rotates — the body topples
//! onto its side instead of sinking flat. A joint constraint then pulls each bone's pivot
//! and the matching spot on its parent together, moving the corners of BOTH boxes
//! (weighted by where the pivot sits in each box and by box volume), so a grounded leg
//! holds the body up and an off-centre load tips it over. Detached ragdolls omit joints.
//! Each joint also has a swing limit: a limb
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

use glam::{Mat3, Mat4, Quat};

use petramond_math::math::{voxel_at, IVec3, Vec3};
use petramond_world::verlet;

use super::model_meta::Skeleton;
use super::RagdollJoints;

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
const LAUNCH_VARIATION: f32 = 0.3;
const SCATTER_SPEED: f32 = 1.25;
const BONE_SPIN: f32 = 4.0;
/// How much the launch tumbles the corpse: the spin's edge velocity is this times the launch speed.
/// Kept below the slowest bone's forward launch so whole-body tumbling cannot pull it
/// back toward the attacker.
const SPIN_FRACTION: f32 = 0.25;
const CORNER_SPIN: f32 = 1.5;
const SEED_DT: f32 = 0.05;
const EPS: f32 = 1e-5;

/// One bone as a rigid body: its 8 box-corner Verlet particles, plus the rest geometry
/// (corner offsets from the rest centroid, the rest centroid, and the pivot) needed to
/// shape-match a rotation and to attach to its parent. `c`/`rot` are the recovered
/// centroid + orientation; the `prev_*` fields snapshot the tick start for interpolation.
///
/// A bone with a weld runs no physics of its own — no integration, no
/// shape matching, no collision, no joint: its `c`/`rot` are derived rigidly from the
/// anchor (its nearest non-welded ancestor) each iteration, and its `nodes` are unused
/// after init.
struct RagBone {
    nodes: [Vec3; 8],
    nodes_old: [Vec3; 8],
    rest: [Vec3; 8],
    c0: Vec3,
    rest_pivot: Vec3,
    inv_mass: f32,
    parent: Option<Joint>,
    weld: Option<Weld>,
    c: Vec3,
    rot: Quat,
    prev_c: Vec3,
    prev_rot: Quat,
}

/// The rest pivot embedded in both boxes as trilinear corner weights, so the joint's
/// two ends are linear in the corners and its correction can rotate either bone.
struct Joint {
    parent: usize,
    in_parent: [f32; 8],
    in_self: [f32; 8],
}

struct Weld {
    anchor: usize,
    offset: Vec3,
    rotation: Quat,
}

pub struct Ragdoll {
    bones: Vec<RagBone>,
    age: f32,
    seed: u64,
    launch: Vec3,
    joints: RagdollJoints,
    impulse_scale: f32,
    // Deltas over the rendered rest pose; physics and rendering use the same bone indices.
    initial_pose: Vec<Mat4>,
    init: bool,
}

impl Ragdoll {
    pub fn pending(
        seed: u64,
        launch: Vec3,
        joints: RagdollJoints,
        impulse_scale: f32,
        initial_pose: Vec<Mat4>,
    ) -> Self {
        Ragdoll {
            bones: Vec::new(),
            age: 0.0,
            seed,
            launch,
            joints,
            impulse_scale,
            initial_pose,
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

    pub fn pending_pose(&self, model: &petramond_world::bbmodel::Model) -> Vec<(Vec3, Quat)> {
        model
            .bones
            .iter()
            .zip(model.rest_pose())
            .enumerate()
            .map(|(i, (bone, rest))| {
                let delta = self.initial_pose.get(i).copied().unwrap_or(Mat4::IDENTITY);
                (
                    delta.transform_point3(rest.transform_point3(bone.pivot)),
                    delta.to_scale_rotation_translation().1.normalize(),
                )
            })
            .collect()
    }

    pub fn init(&mut self, skel: &Skeleton, scale: f32, mob_vel: Vec3, yaw: f32) {
        let initial_pose = std::mem::take(&mut self.initial_pose);
        let delta = |i: usize| initial_pose.get(i).copied().unwrap_or(Mat4::IDENTITY);
        let to_model = Quat::from_rotation_y(-yaw);
        let launch = to_model * self.launch;
        let inherited = (to_model * mob_vel / scale) * 0.4;
        let launch_speed = LAUNCH_SPEED / scale * self.impulse_scale;
        let launch_vel = launch * launch_speed;
        let up = (POP_UP + LAUNCH_UP / scale) * self.impulse_scale;
        let centre = if skel.bones.is_empty() {
            Vec3::ZERO
        } else {
            skel.bones
                .iter()
                .enumerate()
                .map(|(i, b)| delta(i).transform_point3((b.bbox_min + b.bbox_max) * 0.5))
                .sum::<Vec3>()
                / skel.bones.len() as f32
        };
        let radius = skel
            .bones
            .iter()
            .enumerate()
            .flat_map(|(i, b)| {
                corners(b.bbox_min, b.bbox_max).map(|c| delta(i).transform_point3(c))
            })
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
                let c = delta(i).transform_point3(c0);
                let rot = delta(i).to_scale_rotation_translation().1.normalize();
                let (base, spin) = match self.joints {
                    RagdollJoints::Connected => (
                        inherited
                            + launch_vel
                            + Vec3::Y * up
                            + Vec3::new((h(1) - 0.5) * 1.5, h(2) * 0.5, (h(3) - 0.5) * 1.5)
                                * self.impulse_scale,
                        Vec3::ZERO,
                    ),
                    RagdollJoints::Detached => {
                        let spread = if launch.length_squared() > EPS {
                            Vec3::Y.cross(launch) * (h(3) * 2.0 - 1.0)
                        } else {
                            to_model * Vec3::new(h(1) * 2.0 - 1.0, 0.0, h(3) * 2.0 - 1.0)
                        };
                        // Free bodies scatter in metres/second and spin in radians/second.
                        // Seeding that spin on jointed limbs would fight their constraints.
                        (
                            inherited
                                + launch_vel * (1.0 + LAUNCH_VARIATION * (h(1) * 2.0 - 1.0))
                                + Vec3::Y
                                    * (LAUNCH_UP / scale * self.impulse_scale)
                                    * (0.75 + h(2) * 0.5)
                                + spread * (SCATTER_SPEED * self.impulse_scale / scale),
                            to_model
                                * Vec3::new(h(4) - 0.5, h(5) - 0.5, h(6) - 0.5)
                                * (BONE_SPIN * self.impulse_scale),
                        )
                    }
                };
                let mut nodes = [Vec3::ZERO; 8];
                let mut nodes_old = [Vec3::ZERO; 8];
                let mut rest = [Vec3::ZERO; 8];
                for (k, corner) in cs.into_iter().enumerate() {
                    let posed = delta(i).transform_point3(corner);
                    let jitter = match self.joints {
                        RagdollJoints::Connected => {
                            Vec3::new(
                                h(10 + k as u64) - 0.5,
                                h(20 + k as u64) - 0.5,
                                h(30 + k as u64) - 0.5,
                            ) * (CORNER_SPIN * self.impulse_scale)
                        }
                        RagdollJoints::Detached => spin.cross(posed - c),
                    };
                    let v = base + omega.cross(posed - centre) + jitter;
                    nodes[k] = posed;
                    nodes_old[k] = posed - v * SEED_DT;
                    rest[k] = corner - c0;
                }
                RagBone {
                    nodes,
                    nodes_old,
                    rest,
                    c0,
                    rest_pivot: b.pivot,
                    inv_mass: 1.0 / box_volume(b.bbox_min, b.bbox_max),
                    parent: match self.joints {
                        RagdollJoints::Connected => b.parent.map(|p| {
                            // A welded bone has no corners of its own to pull on.
                            let p = anchor_of(p).unwrap_or(p);
                            let pb = &skel.bones[p];
                            Joint {
                                parent: p,
                                in_parent: trilinear(pb.bbox_min, pb.bbox_max, b.pivot),
                                in_self: trilinear(b.bbox_min, b.bbox_max, b.pivot),
                            }
                        }),
                        RagdollJoints::Detached => None,
                    },
                    weld: anchor_of(i).map(|anchor| {
                        let a = &skel.bones[anchor];
                        let ac = delta(anchor).transform_point3((a.bbox_min + a.bbox_max) * 0.5);
                        let ar = delta(anchor).to_scale_rotation_translation().1.normalize();
                        Weld {
                            anchor,
                            offset: ar.inverse() * (c - ac),
                            rotation: (ar.inverse() * rot).normalize(),
                        }
                    }),
                    c,
                    rot,
                    prev_c: c,
                    prev_rot: rot,
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
        // Corners resolve in world space; a corpse falls up to ~2 m per tick late in its
        // lifetime, which is why the resolve walks cell boundaries.
        let resolve = |old: Vec3, cur: Vec3| -> Vec3 {
            let w = verlet::resolve_through_cells(world_of(old), world_of(cur), solid);
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
                verlet::integrate(&mut b.nodes[k], &mut b.nodes_old[k], accel, dt2, VEL_DAMP);
                if solid(voxel_at(world_of(b.nodes[k] - probe))) {
                    let v = b.nodes[k] - b.nodes_old[k];
                    b.nodes_old[k].x = b.nodes[k].x - v.x * GROUND_FRICTION;
                    b.nodes_old[k].z = b.nodes[k].z - v.z * GROUND_FRICTION;
                }
            }
        }

        let max_rot = MAX_ANGULAR_SPEED * dt;
        for _ in 0..ITERS {
            // Shape-match each bone: fit a centroid and rotation to the corner cloud and snap the
            // corners back. So whatever moved the corners (gravity, collisions, joints) ends up
            // rotating the bone.
            for b in &mut self.bones {
                if b.weld.is_none() {
                    b.shape_match(max_rot);
                }
            }
            for i in 0..self.bones.len() {
                if self.bones[i].weld.is_none() {
                    self.limit_swing(i);
                    self.close_joint(i);
                }
            }
            // Collision runs after the joints so no joint can drag a corner into the ground.
            for b in &mut self.bones {
                if b.weld.is_some() {
                    continue;
                }
                for k in 0..8 {
                    b.nodes[k] = resolve(b.nodes_old[k], b.nodes[k]);
                }
            }
        }
        for b in &mut self.bones {
            if b.weld.is_none() {
                b.shape_match(max_rot);
            }
        }
        for i in 0..self.bones.len() {
            if self.bones[i].weld.is_none() {
                self.limit_swing(i);
            }
        }
        // The rigid fit averages collision-resolved corners back into the ground; lift the
        // corpse out as a whole (each piece, when detached) so no rendered box ends a tick
        // inside terrain and no joint is pulled open.
        let pushes: Vec<Vec3> = self
            .bones
            .iter()
            .map(|b| {
                let mut push = Vec3::ZERO;
                if b.weld.is_none() {
                    for k in 0..8 {
                        push = widest(push, resolve(b.nodes_old[k], b.nodes[k]) - b.nodes[k]);
                    }
                }
                push
            })
            .collect();
        let whole = pushes.iter().copied().fold(Vec3::ZERO, widest);
        for (b, push) in self.bones.iter_mut().zip(pushes) {
            let push = match self.joints {
                RagdollJoints::Connected => whole,
                RagdollJoints::Detached => push,
            };
            // Shift the tick-start corners too: a position fix must not become upward velocity.
            b.c += push;
            for (node, old) in b.nodes.iter_mut().zip(&mut b.nodes_old) {
                *node += push;
                *old += push;
            }
        }
        for i in 0..self.bones.len() {
            let Some(weld) = &self.bones[i].weld else {
                continue;
            };
            let anchor = &self.bones[weld.anchor];
            let c = anchor.c + anchor.rot * weld.offset;
            let rot = (anchor.rot * weld.rotation).normalize();
            let b = &mut self.bones[i];
            b.rot = rot;
            b.c = c;
        }
        self.age += dt;
    }

    fn limit_swing(&mut self, i: usize) {
        let Some(joint) = &self.bones[i].parent else {
            return;
        };
        let pr = self.bones[joint.parent].rot;
        let rel = (pr.inverse() * self.bones[i].rot).normalize();
        let lim = clamp_rotation(Quat::IDENTITY, rel, MAX_JOINT_SWING);
        if lim.angle_between(rel) > EPS {
            let b = &mut self.bones[i];
            b.rot = (pr * lim).normalize();
            for k in 0..8 {
                b.nodes[k] = b.c + b.rot * b.rest[k];
            }
        }
    }

    /// Closes the gap between the two ends of bone `i`'s joint by moving both boxes'
    /// corners, each by its weight over its mass.
    fn close_joint(&mut self, i: usize) {
        let Some(joint) = &self.bones[i].parent else {
            return;
        };
        let (p, in_parent, in_self) = (joint.parent, joint.in_parent, joint.in_self);
        let embedded = |b: &RagBone, w: &[f32; 8]| (0..8).map(|k| b.nodes[k] * w[k]).sum::<Vec3>();
        let gap = embedded(&self.bones[i], &in_self) - embedded(&self.bones[p], &in_parent);
        let (mp, mc) = (self.bones[p].inv_mass, self.bones[i].inv_mass);
        let denom = mp * in_parent.iter().map(|w| w * w).sum::<f32>()
            + mc * in_self.iter().map(|w| w * w).sum::<f32>();
        if denom <= EPS {
            return;
        }
        let lambda = gap / denom;
        for k in 0..8 {
            self.bones[p].nodes[k] += lambda * (mp * in_parent[k]);
            self.bones[i].nodes[k] -= lambda * (mc * in_self[k]);
        }
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
                let (c, rot) = match &b.weld {
                    Some(weld) => {
                        let (ac, ar) = interp[weld.anchor];
                        (ac + ar * weld.offset, (ar * weld.rotation).normalize())
                    }
                    None => interp[i],
                };
                (c + rot * (b.rest_pivot - b.c0), rot)
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

/// Per axis, whichever of `a` and `b` reaches further.
fn widest(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::select(b.abs().cmpgt(a.abs()), b, a)
}

/// Weights on [`corners`] that reproduce `p` exactly for any affine placement of the box.
fn trilinear(min: Vec3, max: Vec3, p: Vec3) -> [f32; 8] {
    let t = (p - min) / (max - min).max(Vec3::splat(EPS));
    std::array::from_fn(|k| {
        let axis = |bit: usize, t: f32| if k >> bit & 1 == 1 { t } else { 1.0 - t };
        axis(0, t.x) * axis(1, t.y) * axis(2, t.z)
    })
}

fn box_volume(min: Vec3, max: Vec3) -> f32 {
    (max - min).max(Vec3::splat(EPS)).element_product()
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
