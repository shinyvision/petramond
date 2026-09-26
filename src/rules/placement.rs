//! Placement orientation rules shared by the server's placement consumers and
//! the client's placement prediction.

use petramond_math::facing::Facing;
use petramond_math::math::Vec3;

/// The furnace facing for a block placed while looking along `forward`: the front
/// (mouth) points back toward the player — opposite the camera's horizontal look
/// direction — snapped to the nearest cardinal.
pub fn facing_from_forward(forward: Vec3) -> Facing {
    let (fx, fz) = (-forward.x, -forward.z);
    if fx.abs() >= fz.abs() {
        if fx >= 0.0 {
            Facing::East
        } else {
            Facing::West
        }
    } else if fz >= 0.0 {
        Facing::South
    } else {
        Facing::North
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_faces_back_toward_the_player() {
        assert_eq!(facing_from_forward(Vec3::new(0.0, 0.0, 1.0)), Facing::North);
        assert_eq!(facing_from_forward(Vec3::new(0.0, 0.0, -1.0)), Facing::South);
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
