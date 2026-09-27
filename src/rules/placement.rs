use petramond_math::facing::Facing;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Aabb;

pub fn facing_from_forward(forward: Vec3) -> Facing {
    Facing::toward_viewer(forward)
}

#[derive(Copy, Clone, Debug)]
pub enum Occupant {
    Player {
        feet: WorldPos,
    },
    Mob {
        pos: WorldPos,
        yaw: f32,
        kind: crate::mob::Mob,
    },
}

pub const fn player_occupies(placer: bool, alive: bool, spectator: bool) -> bool {
    placer || (alive && !spectator)
}

pub fn placement_blocked_by_bodies(
    cell: IVec3,
    boxes: &[Aabb],
    occupants: impl IntoIterator<Item = Occupant>,
) -> bool {
    if boxes.is_empty() {
        return false;
    }
    occupants.into_iter().any(|o| match o {
        Occupant::Player { feet } => {
            petramond_world::body::Body::new(feet, crate::player::HALF_W, crate::player::HEIGHT)
                .overlaps_block_boxes(cell, boxes)
        }
        Occupant::Mob { pos, yaw, kind } => {
            crate::mob::body_overlaps_block_boxes(pos, yaw, crate::mob::def(kind).size, cell, boxes)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bodies_that_count_block_and_collisionless_blocks_never_do() {
        let cell = IVec3::new(0, 64, 0);
        let inside = Occupant::Player {
            feet: WorldPos::new(0.5, 64.0, 0.5),
        };
        let away = Occupant::Player {
            feet: WorldPos::new(10.5, 64.0, 10.5),
        };
        let cube = petramond_world::block::Block::Stone.collision_boxes();
        assert!(placement_blocked_by_bodies(cell, cube, [away, inside]));
        assert!(!placement_blocked_by_bodies(cell, cube, [away]));
        assert!(!placement_blocked_by_bodies(cell, &[], [inside]));
        assert!(
            player_occupies(true, false, true),
            "the placer always counts"
        );
        assert!(player_occupies(false, true, false));
        assert!(!player_occupies(false, false, false), "the dead do not");
        assert!(!player_occupies(false, true, true), "spectators do not");
    }

    #[test]
    fn the_front_faces_back_toward_the_player() {
        assert_eq!(facing_from_forward(Vec3::new(0.0, 0.0, 1.0)), Facing::North);
        assert_eq!(
            facing_from_forward(Vec3::new(0.0, 0.0, -1.0)),
            Facing::South
        );
        assert_eq!(facing_from_forward(Vec3::new(1.0, 0.0, 0.0)), Facing::West);
        assert_eq!(facing_from_forward(Vec3::new(-1.0, 0.0, 0.0)), Facing::East);
    }

    #[test]
    fn pitch_is_ignored_and_the_dominant_horizontal_axis_wins() {
        assert_eq!(
            facing_from_forward(Vec3::new(0.2, -0.9, 0.95)),
            Facing::North,
            "a steep look down still snaps by the horizontal component"
        );
        assert_eq!(
            facing_from_forward(Vec3::new(0.5, 0.0, 0.5)),
            Facing::West,
            "a diagonal tie resolves to the x axis"
        );
    }
}
