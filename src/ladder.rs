//! Ladder orientation and panel geometry: how a placed ladder sits in its cell.
//!
//! A ladder is a thin climbable panel mounted on one vertical wall face.
//! *Which wall it hangs on* is block IDENTITY — one block row per [`Facing`]
//! (`Block::panel_facing`, the sapling-stage pattern), committed by placement
//! as the sibling row matching the clicked face's outward normal
//! ([`Facing::from_horizontal_normal`]; vertical normals refuse, so no floor
//! or ceiling ladders). There is no per-cell ladder state: the facing rides
//! the ordinary block-id save/replication lanes, and every reader takes it
//! off the block it already fetched. This module owns the single panel box
//! that the mesher, the raycast target, the selection outline, and the
//! break-crack overlay all build from — so they trace the same geometry by
//! construction.

use crate::block::Aabb;
use crate::facing::Facing;
use crate::mathh::IVec3;

/// Panel thickness in cell units: the ladder occupies the 1/16 slice of its cell
/// flush against the supporting wall.
pub const THICKNESS: f32 = 1.0 / 16.0;

/// The wall cell a ladder at `pos` hangs on: directly behind its panel, opposite
/// the facing.
#[inline]
pub fn support_cell(pos: IVec3, facing: Facing) -> IVec3 {
    pos - facing.dir()
}

/// The panel as collision geometry, per facing. The panel is REAL collision: a
/// body walking along the wall bumps into it, and the top of a ladder column is
/// standable. It is thinner than the movement-claim penetration tolerance, so a
/// body pressed flush against it can never read as deeply penetrating.
static PANEL_NORTH: [Aabb; 1] = [Aabb {
    // Front faces -Z: the wall is the +Z neighbour, the panel hugs z = 1.
    min: [0.0, 0.0, 1.0 - THICKNESS],
    max: [1.0, 1.0, 1.0],
}];
static PANEL_SOUTH: [Aabb; 1] = [Aabb {
    min: [0.0, 0.0, 0.0],
    max: [1.0, 1.0, THICKNESS],
}];
static PANEL_WEST: [Aabb; 1] = [Aabb {
    min: [1.0 - THICKNESS, 0.0, 0.0],
    max: [1.0, 1.0, 1.0],
}];
static PANEL_EAST: [Aabb; 1] = [Aabb {
    min: [0.0, 0.0, 0.0],
    max: [THICKNESS, 1.0, 1.0],
}];

/// The ladder's facing-resolved collision boxes: the 1/16 panel slice against
/// the supporting wall.
pub fn collision_boxes(facing: Facing) -> &'static [Aabb] {
    match facing {
        Facing::North => &PANEL_NORTH,
        Facing::South => &PANEL_SOUTH,
        Facing::West => &PANEL_WEST,
        Facing::East => &PANEL_EAST,
    }
}

/// The panel box for arbitrary `thickness`/`height` (cell fractions) — the ONE
/// geometry a Layer-2 wall-panel derives collision, targeting, and mesh from.
/// The engine ladder is `panel_box(facing, THICKNESS, 1.0)`.
pub fn panel_box(facing: Facing, thickness: f32, height: f32) -> Aabb {
    match facing {
        // Front faces -Z: the wall is the +Z neighbour, the panel hugs z = 1.
        Facing::North => Aabb {
            min: [0.0, 0.0, 1.0 - thickness],
            max: [1.0, height, 1.0],
        },
        Facing::South => Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, height, thickness],
        },
        Facing::West => Aabb {
            min: [1.0 - thickness, 0.0, 0.0],
            max: [1.0, height, 1.0],
        },
        Facing::East => Aabb {
            min: [0.0, 0.0, 0.0],
            max: [thickness, height, 1.0],
        },
    }
}

/// Interned `'static` panel box sets for parameterized wall panels, keyed by
/// `(facing, thickness bits, height bits)` — a handful per distinct panel dim.
static PANEL_INTERN: std::sync::Mutex<Vec<((u8, u32, u32), &'static [Aabb])>> =
    std::sync::Mutex::new(Vec::new());

/// A wall-panel's collision boxes for retuned `thickness`/`height`. The engine
/// default returns the static slice (no alloc); other dims intern once.
pub fn collision_boxes_dim(facing: Facing, thickness: f32, height: f32) -> &'static [Aabb] {
    if thickness == THICKNESS && height == 1.0 {
        return collision_boxes(facing);
    }
    let key = (facing.to_u8(), thickness.to_bits(), height.to_bits());
    let mut intern = PANEL_INTERN.lock().expect("panel intern");
    if let Some(&(_, boxes)) = intern.iter().find(|(k, _)| *k == key) {
        return boxes;
    }
    let leaked: &'static [Aabb] =
        Box::leak(vec![panel_box(facing, thickness, height)].into_boxed_slice());
    intern.push((key, leaked));
    leaked
}

/// The panel's cell-local AABB for `thickness`/`height` — the targeting /
/// outline / crack box, derived from the same [`panel_box`] as collision so
/// they never disagree. The engine ladder is `panel_aabb_dim(facing, THICKNESS,
/// 1.0)`.
pub fn panel_aabb_dim(facing: Facing, thickness: f32, height: f32) -> ([f32; 3], [f32; 3]) {
    let b = panel_box(facing, thickness, height);
    (b.min, b.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_is_the_wall_behind_the_panel() {
        let p = IVec3::new(5, 10, -3);
        // An east-facing ladder's front points +X, so it hangs on the wall at -X.
        assert_eq!(support_cell(p, Facing::East), IVec3::new(4, 10, -3));
        assert_eq!(support_cell(p, Facing::North), IVec3::new(5, 10, -2));
    }

    #[test]
    fn panel_hugs_the_supporting_wall() {
        // Full-height, full-width, THICKNESS-deep slice flush against the wall
        // opposite the facing (an east-facing ladder hangs on its -X wall).
        let t = THICKNESS;
        let aabb = |f| panel_aabb_dim(f, THICKNESS, 1.0);
        assert_eq!(aabb(Facing::East), ([0.0, 0.0, 0.0], [t, 1.0, 1.0]));
        assert_eq!(aabb(Facing::West), ([1.0 - t, 0.0, 0.0], [1.0, 1.0, 1.0]));
        assert_eq!(aabb(Facing::South), ([0.0, 0.0, 0.0], [1.0, 1.0, t]));
        assert_eq!(aabb(Facing::North), ([0.0, 0.0, 1.0 - t], [1.0, 1.0, 1.0]));
    }

    #[test]
    fn parameterized_panel_thickens_and_shortens_the_box() {
        // A Layer-2 wall panel (thickness 4/16, height 12/16) grows the slice and
        // caps its height; the box the collision, targeting, and mesh share.
        let boxes = collision_boxes_dim(Facing::East, 4.0 / 16.0, 12.0 / 16.0);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].min, [0.0, 0.0, 0.0]);
        assert_eq!(boxes[0].max, [4.0 / 16.0, 12.0 / 16.0, 1.0]);
        // The engine default routes to the static, no-alloc slice.
        assert!(std::ptr::eq(
            collision_boxes_dim(Facing::East, THICKNESS, 1.0),
            collision_boxes(Facing::East)
        ));
        // Identical dims intern to the SAME leaked slice (pointer identity).
        assert!(std::ptr::eq(
            collision_boxes_dim(Facing::West, 5.0 / 16.0, 1.0),
            collision_boxes_dim(Facing::West, 5.0 / 16.0, 1.0)
        ));
    }
}
