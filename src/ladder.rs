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

/// The ladder panel's cell-local AABB — the same 1/16 slice as
/// [`collision_boxes`]. One box shared by targeting, the outline, and the
/// crack overlay.
pub fn panel_aabb(facing: Facing) -> ([f32; 3], [f32; 3]) {
    let b = &collision_boxes(facing)[0];
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
        assert_eq!(panel_aabb(Facing::East), ([0.0, 0.0, 0.0], [t, 1.0, 1.0]));
        assert_eq!(
            panel_aabb(Facing::West),
            ([1.0 - t, 0.0, 0.0], [1.0, 1.0, 1.0])
        );
        assert_eq!(panel_aabb(Facing::South), ([0.0, 0.0, 0.0], [1.0, 1.0, t]));
        assert_eq!(
            panel_aabb(Facing::North),
            ([0.0, 0.0, 1.0 - t], [1.0, 1.0, 1.0])
        );
    }
}
