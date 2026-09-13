//! Shared gameplay body geometry for entity-like things.
//!
//! A body is an upright AABB used by gameplay systems that need entity
//! occupancy: soft entity pushing, placement blocking, targeting/reach checks,
//! and similar rules. The body is only geometry; each caller decides which
//! bodies participate in its rule.

use crate::block::Aabb;
use crate::mathh::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;

/// Boundary epsilon in world units. Face-touching boxes are not overlapping;
/// only genuine interpenetration counts.
const EPS: f64 = 1e-4;

/// Push speed imparted per metre of overlap (1/s): a body overlapping another by
/// `overlap` metres is pushed off it at `overlap * PUSH_STRENGTH` m/s this tick.
const PUSH_STRENGTH: f32 = 4.0;

/// An entity-like gameplay body: an upright box `hw` half-wide on X and Z,
/// centred horizontally at `(x, z)`, spanning `[y0, y1]`.
#[derive(Copy, Clone, Debug)]
pub struct Body {
    pub x: f64,
    pub z: f64,
    pub y0: f64,
    pub y1: f64,
    pub hw: f32,
}

impl Body {
    /// A body with feet at `pos`, `height` tall and `hw` half-wide.
    pub fn new(pos: WorldPos, hw: f32, height: f32) -> Self {
        Body {
            x: pos.x,
            z: pos.z,
            y0: pos.y,
            y1: pos.y + f64::from(height),
            hw,
        }
    }

    /// World-space min/max corners.
    pub fn aabb(self) -> ([f64; 3], [f64; 3]) {
        let hw = f64::from(self.hw);
        (
            [self.x - hw, self.y0, self.z - hw],
            [self.x + hw, self.y1, self.z + hw],
        )
    }

    /// Whether this body overlaps any supplied cell-local block collision box.
    pub fn overlaps_block_boxes(self, cell: IVec3, boxes: &[Aabb]) -> bool {
        let (amin, amax) = self.aabb();
        let origin = [cell.x, cell.y, cell.z].map(f64::from);
        boxes.iter().any(|b| {
            let bmin: [f64; 3] = std::array::from_fn(|i| origin[i] + f64::from(b.min[i]));
            let bmax: [f64; 3] = std::array::from_fn(|i| origin[i] + f64::from(b.max[i]));
            aabb_overlaps((amin, amax), (bmin, bmax))
        })
    }
}

/// Strict AABB overlap with a small epsilon, so face-touching boxes are allowed.
fn aabb_overlaps((amin, amax): ([f64; 3], [f64; 3]), (bmin, bmax): ([f64; 3], [f64; 3])) -> bool {
    (0..3).all(|i| amin[i] < bmax[i] - EPS && bmin[i] < amax[i] - EPS)
}

/// The horizontal push velocity (m/s) to add to body `a` this tick to ease it
/// off body `b`, or `None` if the two do not overlap.
pub fn separation(a: Body, b: Body) -> Option<Vec3> {
    // Vertical spans must overlap; otherwise one is stacked above the other and
    // gets no sideways push.
    if a.y1 <= b.y0 || b.y1 <= a.y0 {
        return None;
    }
    let dx = (a.x - b.x) as f32;
    let dz = (a.z - b.z) as f32;
    let reach = a.hw + b.hw;
    let overlap_x = reach - dx.abs();
    let overlap_z = reach - dz.abs();
    if overlap_x <= EPS as f32 || overlap_z <= EPS as f32 {
        return None;
    }

    // The bodies are AABBs, so diagonal corner overlap is still contact. Keep
    // the established centre-line shove direction, but scale it by the
    // shallowest axis penetration so segment depth remains comparable.
    let dist_sq = dx * dx + dz * dz;
    let dist = dist_sq.sqrt();
    let overlap = overlap_x.min(overlap_z);
    // Exactly coincident centres have no defined direction. Split along +X so
    // perfectly stacked bodies still separate deterministically.
    let (nx, nz) = if dist > EPS as f32 {
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
    fn body(x: f64, y: f64, z: f64) -> Body {
        Body::new(WorldPos::new(x, y, z), 0.25, 1.0)
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
