//! A rigid ROTATION about a pivot — how a cell-local box is posed off the
//! axis grid. The one transform every consumer of a posed box goes through
//! (mesh corners, the item form, the crack overlay, targeting, occupancy),
//! so a box cannot render where it is not aimed or shadow where it is not
//! drawn.

use glam::{Quat, Vec3};

/// A rotation `rotation` about the cell-local point `origin`. Applied to a
/// box authored axis-aligned, it yields an oriented box (OBB) whose faces
/// keep their authored order and art.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BoxPose {
    pub rotation: Quat,
    pub origin: Vec3,
}

/// The cell centre a quarter turn about Y pivots on.
const CELL_CENTRE: Vec3 = Vec3::new(0.5, 0.5, 0.5);

impl BoxPose {
    /// A rotation from Blockbench-style euler DEGREES, composed X first, then
    /// Y, then Z — the outliner-node order every `.bbmodel` cube uses, so a
    /// cube transcribed from a model file poses identically here.
    pub fn from_euler_degrees(deg: [f32; 3], origin: [f32; 3]) -> Self {
        BoxPose {
            rotation: Quat::from_rotation_z(deg[2].to_radians())
                * Quat::from_rotation_y(deg[1].to_radians())
                * Quat::from_rotation_x(deg[0].to_radians()),
            origin: Vec3::from(origin),
        }
    }

    /// Local (authored) point → posed point.
    #[inline]
    pub fn apply(&self, p: Vec3) -> Vec3 {
        self.origin + self.rotation * (p - self.origin)
    }

    /// Posed point → local (authored) point.
    #[inline]
    pub fn unapply(&self, p: Vec3) -> Vec3 {
        self.origin + self.rotation.inverse() * (p - self.origin)
    }

    /// A direction through the pose (no translation).
    #[inline]
    pub fn rotate(&self, d: Vec3) -> Vec3 {
        self.rotation * d
    }

    /// This pose after the whole cell is turned one quarter about its Y
    /// centre, in the same sense a box set's `turned()` takes its axis-aligned
    /// boxes: `(x, z) -> (1 - z, x)`. The turn composes onto the rotation and
    /// carries the pivot, so a posed box in a turned shape lands exactly where
    /// turning its posed corners would put it.
    pub fn turned(&self) -> Self {
        let turn = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        BoxPose {
            rotation: turn * self.rotation,
            origin: CELL_CENTRE + turn * (self.origin - CELL_CENTRE),
        }
    }

    /// The axis-aligned bounds of the local box `[min, max]` once posed.
    pub fn bounds(&self, min: [f32; 3], max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for x in [min[0], max[0]] {
            for y in [min[1], max[1]] {
                for z in [min[2], max[2]] {
                    let p = self.apply(Vec3::new(x, y, z));
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
        (lo.to_array(), hi.to_array())
    }

    /// Whether the posed local box `[min, max]` overlaps the axis-aligned
    /// pocket `[lo, hi]` with positive volume — the separating-axis test over
    /// the three world axes, the three box axes, and their nine cross
    /// products. A graze (touching faces) is not an overlap, matching the
    /// strict test axis-aligned boxes use.
    pub fn overlaps_aabb(&self, min: [f32; 3], max: [f32; 3], lo: [f32; 3], hi: [f32; 3]) -> bool {
        const EPS: f32 = 1e-5;
        let bmin = Vec3::from(min);
        let bmax = Vec3::from(max);
        let pmin = Vec3::from(lo);
        let pmax = Vec3::from(hi);
        // Both boxes as centre + half-extent; the pocket in world axes, the
        // box in its own rotated axes.
        let bc = self.apply((bmin + bmax) * 0.5);
        let bh = (bmax - bmin) * 0.5;
        let pc = (pmin + pmax) * 0.5;
        let ph = (pmax - pmin) * 0.5;
        let axes = [
            self.rotation * Vec3::X,
            self.rotation * Vec3::Y,
            self.rotation * Vec3::Z,
        ];
        let d = bc - pc;
        let separated = |axis: Vec3| -> bool {
            let len = axis.length();
            if len < EPS {
                // A degenerate cross product (parallel edges): this axis is
                // covered by the face axes already.
                return false;
            }
            let n = axis / len;
            let rb = bh.x * n.dot(axes[0]).abs()
                + bh.y * n.dot(axes[1]).abs()
                + bh.z * n.dot(axes[2]).abs();
            let rp = ph.x * n.x.abs() + ph.y * n.y.abs() + ph.z * n.z.abs();
            d.dot(n).abs() + EPS >= rb + rp
        };
        for n in [Vec3::X, Vec3::Y, Vec3::Z] {
            if separated(n) {
                return false;
            }
        }
        for a in axes {
            if separated(a) {
                return false;
            }
        }
        for w in [Vec3::X, Vec3::Y, Vec3::Z] {
            for a in axes {
                if separated(w.cross(a)) {
                    return false;
                }
            }
        }
        true
    }

    /// First crossing of the ray `eye + dir·t` (both in the box's cell frame)
    /// through the posed local box `[min, max]`: the distance and the crossed
    /// face's outward WORLD normal. The ray is carried into the box's frame
    /// (a rigid transform, so `t` is preserved) and slab-tested there; a
    /// zero-thickness plane crosses like any box.
    pub fn ray_hit(
        &self,
        eye: Vec3,
        dir: Vec3,
        min: [f32; 3],
        max: [f32; 3],
    ) -> Option<(f32, Vec3)> {
        const EPS: f32 = 1e-6;
        let e = self.unapply(eye);
        let d = self.rotation.inverse() * dir;
        let mut t_near = f32::NEG_INFINITY;
        let mut t_far = f32::INFINITY;
        let mut normal = Vec3::ZERO;
        for i in 0..3 {
            if d[i].abs() < EPS {
                if e[i] < min[i] - EPS || e[i] > max[i] + EPS {
                    return None;
                }
                continue;
            }
            let inv = 1.0 / d[i];
            let mut t1 = (min[i] - e[i]) * inv;
            let mut t2 = (max[i] - e[i]) * inv;
            let mut n1 = -1.0;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
                n1 = 1.0;
            }
            if t1 > t_near {
                t_near = t1;
                normal = Vec3::ZERO;
                normal[i] = n1;
            }
            t_far = t_far.min(t2);
            if t_near > t_far {
                return None;
            }
        }
        if t_far < 0.0 {
            return None;
        }
        let t = t_near.max(0.0);
        Some((t, self.rotation * normal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    /// Turning a pose must land a box where turning its posed corners would:
    /// the composition, not just the rotation, has to carry the pivot.
    #[test]
    fn a_turned_pose_matches_turning_the_posed_points() {
        let pose = BoxPose::from_euler_degrees([45.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
        let turn = |p: Vec3| Vec3::new(1.0 - p.z, p.y, p.x);
        for p in [
            Vec3::new(0.1, 0.2, 0.3),
            Vec3::new(0.9, 0.5, 0.05),
            Vec3::new(0.0, 1.0, 1.0),
        ] {
            assert!(
                close(pose.turned().apply(p), turn(pose.apply(p))),
                "{p:?}: {:?} vs {:?}",
                pose.turned().apply(p),
                turn(pose.apply(p))
            );
        }
        // Four turns are the identity.
        let four = pose.turned().turned().turned().turned();
        let p = Vec3::new(0.3, 0.6, 0.8);
        assert!(close(four.apply(p), pose.apply(p)));
    }

    /// The SAT overlap: a tilted plane through the cell centre meets a
    /// pocket on the diagonal it crosses and misses one it clears; touching
    /// faces do not count.
    #[test]
    fn overlap_follows_the_posed_geometry_not_the_bounds() {
        // A flat plane at y = 0.5 tilted 45° about X: it passes through the
        // cell's z = 0.5 line and rises toward -Z.
        let pose = BoxPose::from_euler_degrees([45.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
        let (min, max) = ([0.0, 0.5, -0.2], [1.0, 0.5, 1.2]);
        // A pocket high in the +Z corner: the plane is LOW there, so miss.
        assert!(!pose.overlaps_aabb(min, max, [0.4, 0.9, 0.9], [0.6, 1.0, 1.0]));
        // Its bounding box would have said yes.
        let (bmin, bmax) = pose.bounds(min, max);
        assert!(bmin[1] < 0.1 && bmax[1] > 0.9);
        // A pocket high in the -Z corner is on the plane's high side: hit.
        assert!(pose.overlaps_aabb(min, max, [0.4, 0.85, 0.05], [0.6, 0.95, 0.15]));
        // An unposed box grazing a pocket is not an overlap.
        let flat = BoxPose::from_euler_degrees([0.0; 3], [0.5; 3]);
        assert!(!flat.overlaps_aabb([0.0; 3], [0.5; 3], [0.5, 0.0, 0.0], [1.0; 3]));
        assert!(flat.overlaps_aabb([0.0; 3], [0.5; 3], [0.4, 0.0, 0.0], [1.0; 3]));
    }

    /// A ray hits the posed box where the posed geometry is, with the face
    /// normal carried through the rotation.
    #[test]
    fn ray_crosses_the_posed_box_and_reports_the_rotated_normal() {
        let pose = BoxPose::from_euler_degrees([0.0, 45.0, 0.0], [0.5, 0.5, 0.5]);
        let (min, max) = ([0.25, 0.0, 0.25], [0.75, 1.0, 0.75]);
        // Straight down through the centre: hits the top at y = 1.
        let hit = pose
            .ray_hit(Vec3::new(0.5, 3.0, 0.5), Vec3::NEG_Y, min, max)
            .expect("centre ray hits");
        assert!((hit.0 - 2.0).abs() < 1e-4);
        assert!(close(hit.1, Vec3::Y));
        // The square's corner now points along +X (a 45° turn), reaching
        // 0.5 + 0.25·√2 ≈ 0.854: a ray at x = 0.8 crosses it, x = 0.9 misses.
        assert!(pose
            .ray_hit(Vec3::new(0.8, 3.0, 0.5), Vec3::NEG_Y, min, max)
            .is_some());
        assert!(pose
            .ray_hit(Vec3::new(0.9, 3.0, 0.5), Vec3::NEG_Y, min, max)
            .is_none());
        // A side hit's normal is the rotated face normal, not an axis.
        let side = pose
            .ray_hit(Vec3::new(3.0, 0.5, 0.55), Vec3::NEG_X, min, max)
            .expect("side ray hits the corner-on face");
        assert!((side.1.length() - 1.0).abs() < 1e-4);
        assert!(side.1.x > 0.7 && side.1.z.abs() > 0.7 - 1e-3);
    }
}
