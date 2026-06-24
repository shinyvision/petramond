//! Soft entity separation — the "mobs and the player jostle each other apart" rule,
//! modelled on Minecraft's entity push.
//!
//! Unlike a block, an entity has no solid collision box: two entities (mob↔mob or
//! mob↔player) may freely occupy the same space. Each tick, any overlapping pair adds a
//! small *velocity* to each body, directed along the horizontal line between their
//! centres and **growing the deeper they overlap** — so two bodies that have walked into
//! each other ease apart on their own, the harder the closer they are. Because it is a
//! velocity (carried by each body's own movement + friction, not a teleport), the motion
//! is smooth: bodies drift apart gradually instead of snapping, and there is no jitter
//! from fighting their locomotion.
//!
//! Pure and world-agnostic: it yields the push *velocity* from body extents. Each body
//! applies it through its normal collision-resolved move, so a push can't shove a body
//! through terrain.

use crate::mathh::Vec3;

/// Push speed imparted per metre of overlap (1/s): a body overlapping another by
/// `overlap` metres is pushed off it at `overlap * PUSH_STRENGTH` m/s this tick. So the
/// push is gentle as bodies barely touch and firmer the deeper they interpenetrate
/// (closer ⇒ more), and — being proportional to the overlap — eases smoothly to zero as
/// they separate rather than cutting out abruptly. Tuned for a Minecraft-like jostle:
/// firm enough to part a cluster, gentle enough that you still walk through a mob.
const PUSH_STRENGTH: f32 = 4.0;

/// An entity's body for pushing: an upright box `hw` half-wide on X and Z, centred
/// horizontally at `(x, z)`, spanning the vertical range `[y0, y1]`. Mobs and the
/// player both project to this, so the push rule treats every entity uniformly.
#[derive(Copy, Clone, Debug)]
pub struct Body {
    pub x: f32,
    pub z: f32,
    pub y0: f32,
    pub y1: f32,
    pub hw: f32,
}

impl Body {
    /// A body with feet at `pos`, `height` tall and `hw` half-wide — the mob/player
    /// AABB convention (the position is the feet; the body extends up from there).
    pub fn new(pos: Vec3, hw: f32, height: f32) -> Self {
        Body {
            x: pos.x,
            z: pos.z,
            y0: pos.y,
            y1: pos.y + height,
            hw,
        }
    }
}

/// The horizontal push *velocity* (m/s) to add to body `a` this tick to ease it off body
/// `b`, or `None` if the two don't overlap. Body `b` takes the opposite (`-`) push on its
/// own pass — each is pushed at the full speed (like Minecraft, where both entities get
/// the full nudge), so they separate at the sum. The speed grows with how deeply they
/// overlap (`overlap * PUSH_STRENGTH`). Two bodies disjoint in height (one cleanly above
/// the other) don't push, nor do footprints clear of each other.
pub fn separation(a: Body, b: Body) -> Option<Vec3> {
    // Vertical spans must overlap; otherwise one is stacked above the other (a mob
    // perched on another's back / under the player's feet) and gets no sideways push.
    if a.y1 <= b.y0 || b.y1 <= a.y0 {
        return None;
    }
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    let reach = a.hw + b.hw;
    let dist_sq = dx * dx + dz * dz;
    if dist_sq >= reach * reach {
        return None; // footprints clear of each other — no overlap to resolve
    }
    let dist = dist_sq.sqrt();
    let overlap = reach - dist;
    // Unit direction from b to a (push a away from b). Exactly coincident centres
    // (dist ≈ 0) have no defined direction — split along +X so perfectly-stacked bodies
    // still come apart, deterministically (no RNG) to keep the sim reproducible.
    let (nx, nz) = if dist > 1e-4 {
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

    /// A unit-ish body (half-width 0.25, 1 tall) with feet at `(x, y, z)`.
    fn body(x: f32, y: f32, z: f32) -> Body {
        Body::new(Vec3::new(x, y, z), 0.25, 1.0)
    }

    #[test]
    fn clear_footprints_do_not_push() {
        // Centres 1.0 apart, combined reach 0.5 — well clear, so no push either way.
        assert!(separation(body(0.0, 0.0, 0.0), body(1.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn vertically_disjoint_bodies_do_not_push() {
        // Same column, but b sits a full body-height above a (a's [0,1], b's [2,3]).
        let a = body(0.0, 0.0, 0.0);
        let b = body(0.05, 2.0, 0.0);
        assert!(separation(a, b).is_none(), "stacked, not side-by-side: no push");
    }

    #[test]
    fn overlapping_bodies_push_apart_along_their_centre_line() {
        // b is just east of a (along +X) and overlapping; a is pushed west, b east.
        let a = body(0.0, 0.0, 0.0);
        let b = body(0.3, 0.0, 0.0); // dist 0.3 < reach 0.5 → overlap 0.2
        let pa = separation(a, b).expect("overlap pushes");
        assert!(pa.x < 0.0 && pa.z == 0.0, "a is pushed -X off b: {pa:?}");
        assert_eq!(pa.y, 0.0, "pushing is horizontal only");
        // Speed grows with overlap: 0.2 overlap → 0.2 * PUSH_STRENGTH m/s.
        assert!((pa.x.abs() - 0.2 * PUSH_STRENGTH).abs() < 1e-5, "speed ∝ overlap: {}", pa.x);
        // Symmetric: b takes the exact opposite push (both get the full speed).
        let pb = separation(b, a).expect("overlap pushes");
        assert!((pb.x + pa.x).abs() < 1e-6 && (pb.z + pa.z).abs() < 1e-6, "equal and opposite");
    }

    #[test]
    fn deeper_overlap_pushes_harder() {
        // The closer the bodies (the deeper the overlap), the stronger the push.
        let a = body(0.0, 0.0, 0.0);
        let shallow = separation(a, body(0.4, 0.0, 0.0)).unwrap().length(); // overlap 0.1
        let deep = separation(a, body(0.1, 0.0, 0.0)).unwrap().length(); // overlap 0.4
        assert!(deep > shallow, "closer ⇒ more push: {deep} vs {shallow}");
    }

    #[test]
    fn coincident_centres_still_separate_deterministically() {
        // Exactly stacked bodies have no centre line; the rule must still split them, and
        // the same way every time (reproducible sim).
        let a = body(5.0, 0.0, 5.0);
        let b = body(5.0, 0.0, 5.0);
        let p1 = separation(a, b).expect("coincident bodies are maximally overlapped");
        let p2 = separation(a, b).expect("coincident bodies are maximally overlapped");
        assert_eq!(p1, p2, "deterministic fallback direction");
        assert!(p1.length() > 0.0, "they are actually pushed apart");
    }
}
