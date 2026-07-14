//! Ladder orientation and panel geometry: how a placed ladder sits in its cell.
//!
//! A ladder is a thin climbable panel mounted on one vertical wall face. Its only
//! per-instance state is *which wall it hangs on*, stored as the shared 4-way
//! [`Facing`] in the owning section's entity-facing map (the same map chest and
//! furnace fronts use) — the facing names the direction the panel's FRONT points,
//! away from the supporting wall, matching the clicked face's outward normal.
//! This module owns the facing↔normal mapping and the single panel box that the
//! mesher, the raycast target, the selection outline, and the break-crack overlay
//! all build from — so they trace the same geometry by construction.

use crate::block::Aabb;
use crate::facing::Facing;
use crate::mathh::IVec3;

/// Panel thickness in cell units: the ladder occupies the 1/16 slice of its cell
/// flush against the supporting wall.
pub const THICKNESS: f32 = 1.0 / 16.0;

/// The horizontal unit direction a facing's front points toward (away from the
/// supporting wall, back toward the player who placed it).
#[inline]
pub fn facing_dir(facing: Facing) -> IVec3 {
    match facing {
        Facing::North => IVec3::new(0, 0, -1),
        Facing::South => IVec3::new(0, 0, 1),
        Facing::West => IVec3::new(-1, 0, 0),
        Facing::East => IVec3::new(1, 0, 0),
    }
}

/// The facing for a ladder placed against the face whose outward normal (pointing
/// back toward the player, as the raycast reports it) is `normal`. Only vertical
/// wall faces accept a ladder — a floor (`+Y`), ceiling (`-Y`), or degenerate zero
/// normal yields `None`.
pub fn facing_from_place_normal(normal: IVec3) -> Option<Facing> {
    match (normal.x, normal.y, normal.z) {
        (1, 0, 0) => Some(Facing::East),
        (-1, 0, 0) => Some(Facing::West),
        (0, 0, 1) => Some(Facing::South),
        (0, 0, -1) => Some(Facing::North),
        _ => None,
    }
}

/// The wall cell a ladder at `pos` hangs on: directly behind its panel, opposite
/// the facing.
#[inline]
pub fn support_cell(pos: IVec3, facing: Facing) -> IVec3 {
    pos - facing_dir(facing)
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
    fn place_normal_maps_walls_and_rejects_floor_and_ceiling() {
        assert_eq!(
            facing_from_place_normal(IVec3::new(1, 0, 0)),
            Some(Facing::East)
        );
        assert_eq!(
            facing_from_place_normal(IVec3::new(0, 0, -1)),
            Some(Facing::North)
        );
        assert_eq!(facing_from_place_normal(IVec3::new(0, 1, 0)), None);
        assert_eq!(facing_from_place_normal(IVec3::new(0, -1, 0)), None);
        assert_eq!(facing_from_place_normal(IVec3::ZERO), None);
    }

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
