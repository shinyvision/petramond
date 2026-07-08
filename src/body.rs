//! Shared gameplay body geometry for entity-like things.
//!
//! A body is an upright AABB used by gameplay systems that need entity
//! occupancy: soft entity pushing, placement blocking, targeting/reach checks,
//! and similar rules. The body is only geometry; each caller decides which
//! bodies participate in its rule.

use crate::block::Aabb;
use crate::mathh::{IVec3, Vec3};

/// Boundary epsilon in world units. Face-touching boxes are not overlapping;
/// only genuine interpenetration counts.
const EPS: f32 = 1e-4;

/// Push speed imparted per metre of overlap (1/s): a body overlapping another by
/// `overlap` metres is pushed off it at `overlap * PUSH_STRENGTH` m/s this tick.
const PUSH_STRENGTH: f32 = 4.0;

/// An entity-like gameplay body: an upright box `hw` half-wide on X and Z,
/// centred horizontally at `(x, z)`, spanning `[y0, y1]`.
#[derive(Copy, Clone, Debug)]
pub struct Body {
    pub x: f32,
    pub z: f32,
    pub y0: f32,
    pub y1: f32,
    pub hw: f32,
}

impl Body {
    /// A body with feet at `pos`, `height` tall and `hw` half-wide.
    pub fn new(pos: Vec3, hw: f32, height: f32) -> Self {
        Body {
            x: pos.x,
            z: pos.z,
            y0: pos.y,
            y1: pos.y + height,
            hw,
        }
    }

    /// World-space min/max corners.
    pub fn aabb(self) -> (Vec3, Vec3) {
        (
            Vec3::new(self.x - self.hw, self.y0, self.z - self.hw),
            Vec3::new(self.x + self.hw, self.y1, self.z + self.hw),
        )
    }

    /// Whether this body overlaps any supplied cell-local block collision box.
    pub fn overlaps_block_boxes(self, cell: IVec3, boxes: &[Aabb]) -> bool {
        let (amin, amax) = self.aabb();
        let origin = Vec3::new(cell.x as f32, cell.y as f32, cell.z as f32);
        boxes.iter().any(|b| {
            let bmin = origin + Vec3::new(b.min[0], b.min[1], b.min[2]);
            let bmax = origin + Vec3::new(b.max[0], b.max[1], b.max[2]);
            aabb_overlaps((amin, amax), (bmin, bmax))
        })
    }
}

/// Strict AABB overlap with a small epsilon, so face-touching boxes are allowed.
fn aabb_overlaps((amin, amax): (Vec3, Vec3), (bmin, bmax): (Vec3, Vec3)) -> bool {
    amin.x < bmax.x - EPS
        && bmin.x < amax.x - EPS
        && amin.y < bmax.y - EPS
        && bmin.y < amax.y - EPS
        && amin.z < bmax.z - EPS
        && bmin.z < amax.z - EPS
}

/// The horizontal push velocity (m/s) to add to body `a` this tick to ease it
/// off body `b`, or `None` if the two do not overlap.
pub fn separation(a: Body, b: Body) -> Option<Vec3> {
    // Vertical spans must overlap; otherwise one is stacked above the other and
    // gets no sideways push.
    if a.y1 <= b.y0 || b.y1 <= a.y0 {
        return None;
    }
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    let reach = a.hw + b.hw;
    let dist_sq = dx * dx + dz * dz;
    if dist_sq >= reach * reach {
        return None;
    }
    let dist = dist_sq.sqrt();
    let overlap = reach - dist;
    // Exactly coincident centres have no defined direction. Split along +X so
    // perfectly stacked bodies still separate deterministically.
    let (nx, nz) = if dist > EPS {
        (dx / dist, dz / dist)
    } else {
        (1.0, 0.0)
    };
    let speed = overlap * PUSH_STRENGTH;
    Some(Vec3::new(nx * speed, 0.0, nz * speed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_CUBE: &[Aabb] = &[Aabb {
        min: [0.0, 0.0, 0.0],
        max: [1.0, 1.0, 1.0],
    }];

    /// A unit-ish body (half-width 0.25, 1 tall) with feet at `(x, y, z)`.
    fn body(x: f32, y: f32, z: f32) -> Body {
        Body::new(Vec3::new(x, y, z), 0.25, 1.0)
    }

    #[test]
    fn clear_footprints_do_not_push() {
        assert!(separation(body(0.0, 0.0, 0.0), body(1.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn vertically_disjoint_bodies_do_not_push() {
        let a = body(0.0, 0.0, 0.0);
        let b = body(0.05, 2.0, 0.0);
        assert!(
            separation(a, b).is_none(),
            "stacked, not side-by-side: no push"
        );
    }

    #[test]
    fn overlapping_bodies_push_apart_along_their_centre_line() {
        let a = body(0.0, 0.0, 0.0);
        let b = body(0.3, 0.0, 0.0);
        let pa = separation(a, b).expect("overlap pushes");
        assert!(pa.x < 0.0 && pa.z == 0.0, "a is pushed -X off b: {pa:?}");
        assert_eq!(pa.y, 0.0, "pushing is horizontal only");
        assert!(
            (pa.x.abs() - 0.2 * PUSH_STRENGTH).abs() < 1e-5,
            "speed is proportional to overlap: {}",
            pa.x
        );
        let pb = separation(b, a).expect("overlap pushes");
        assert!(
            (pb.x + pa.x).abs() < 1e-6 && (pb.z + pa.z).abs() < 1e-6,
            "equal and opposite"
        );
    }

    #[test]
    fn deeper_overlap_pushes_harder() {
        let a = body(0.0, 0.0, 0.0);
        let shallow = separation(a, body(0.4, 0.0, 0.0)).unwrap().length();
        let deep = separation(a, body(0.1, 0.0, 0.0)).unwrap().length();
        assert!(
            deep > shallow,
            "closer means more push: {deep} vs {shallow}"
        );
    }

    #[test]
    fn coincident_centres_still_separate_deterministically() {
        let a = body(5.0, 0.0, 5.0);
        let b = body(5.0, 0.0, 5.0);
        let p1 = separation(a, b).expect("coincident bodies overlap");
        let p2 = separation(a, b).expect("coincident bodies overlap");
        assert_eq!(p1, p2, "deterministic fallback direction");
        assert!(p1.length() > 0.0, "they are actually pushed apart");
    }

    #[test]
    fn block_box_overlap_requires_interpenetration() {
        assert!(
            body(0.5, 64.0, 0.5).overlaps_block_boxes(IVec3::new(0, 64, 0), FULL_CUBE),
            "body inside the cube overlaps"
        );
        assert!(
            !body(1.3, 64.0, 0.5).overlaps_block_boxes(IVec3::new(0, 64, 0), FULL_CUBE),
            "face-touching is not overlap"
        );
        assert!(
            !body(0.5, 65.0, 0.5).overlaps_block_boxes(IVec3::new(0, 64, 0), FULL_CUBE),
            "standing exactly on top is not overlap"
        );
    }
}
