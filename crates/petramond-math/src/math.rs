//! Misc math helpers not covered by glam.

pub use glam::{IVec3, Mat4, Quat, Vec3, Vec4};

/// A body's tilt off level, applied INSIDE its yaw: `pitch` about the
/// lateral axis (radians, positive = nose up) and `roll` about the facing
/// axis (radians, positive = right side up). A body's frame is
/// `Ry(yaw) · Rx(pitch) · Rz(roll)` everywhere it is rendered, seated on, or
/// replicated. Every body the engine moves itself is [`LEVEL`](Self::LEVEL);
/// a constrained body (a cart on a slope, a boat on a swell) is given one by
/// whoever constrains it.
#[derive(Copy, Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tilt {
    pub pitch: f32,
    pub roll: f32,
}

impl Tilt {
    pub const LEVEL: Tilt = Tilt {
        pitch: 0.0,
        roll: 0.0,
    };

    pub const fn new(pitch: f32, roll: f32) -> Tilt {
        Tilt { pitch, roll }
    }

    pub fn is_level(self) -> bool {
        self == Tilt::LEVEL
    }

    pub fn is_finite(self) -> bool {
        self.pitch.is_finite() && self.roll.is_finite()
    }

    /// Straight interpolation: tilts are small, bounded angles, never wrapped.
    pub fn lerp(self, to: Tilt, t: f32) -> Tilt {
        Tilt {
            pitch: self.pitch + (to.pitch - self.pitch) * t,
            roll: self.roll + (to.roll - self.roll) * t,
        }
    }

    /// Ease toward level by at most `max_step` radians on each axis.
    pub fn toward_level(self, max_step: f32) -> Tilt {
        Tilt {
            pitch: self.pitch - self.pitch.clamp(-max_step, max_step),
            roll: self.roll - self.roll.clamp(-max_step, max_step),
        }
    }

    /// The rotation inside the yaw: `Rx(pitch) · Rz(roll)`. Built through
    /// [`crate::detmath`], like [`body_frame`](Self::body_frame): a
    /// replicated frame must be the same matrix on every peer.
    pub fn rotation(self) -> Mat4 {
        crate::detmath::mat4_rotation_x(self.pitch) * crate::detmath::mat4_rotation_z(self.roll)
    }

    /// The whole body frame for a mob-convention `yaw` (`0` faces `-Z`):
    /// `Ry(yaw) · Rx(pitch) · Rz(roll)`.
    pub fn body_frame(self, yaw: f32) -> Mat4 {
        crate::detmath::mat4_rotation_y(yaw) * self.rotation()
    }
}

/// The six axis-aligned face-neighbour offsets in canonical face order
/// (`+X, -X, +Y, -Y, +Z, -Z`) — the one shared cardinal-direction table.
/// `mesh::Face::ALL` lists faces in this same order and `Face::dir` indexes
/// into this table, so face/offset correspondence holds by construction.
pub const FACE_NEIGHBORS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

pub const MAX_SELECTION_BOXES: usize = 3;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SelectionBoxes {
    pub boxes: [(Vec3, Vec3); MAX_SELECTION_BOXES],
    pub len: u8,
}

impl SelectionBoxes {
    #[inline]
    pub fn iter(self) -> impl Iterator<Item = (Vec3, Vec3)> {
        self.boxes.into_iter().take(self.len as usize)
    }
}

/// A selection outline, anchored at an integer cell: every variant's geometry
/// is local to `origin`, so the outline stays exact however far out it is.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SelectionShape {
    Box {
        origin: IVec3,
        min: Vec3,
        max: Vec3,
    },
    /// A torch pole. The outline's box corners are `transform`-mapped from the
    /// torch's local model box and offset by `origin` (the cell), so the wireframe
    /// traces the rendered pole — straight for a floor torch, tilted for a wall one.
    /// `transform` is the torch's model transform (`TorchPlacement::model_transform`);
    /// kept as a plain `Mat4` so this generic math type stays torch-agnostic.
    Torch {
        origin: IVec3,
        transform: Mat4,
    },
    /// A shape made from a small fixed list of boxes local to `origin`. Used for
    /// stairs so the outline traces the solid stair volume instead of a full block cube.
    Boxes {
        origin: IVec3,
        boxes: SelectionBoxes,
    },
}

impl SelectionShape {
    pub fn full_block(block: IVec3) -> Self {
        Self::Box {
            origin: block,
            min: Vec3::ZERO,
            max: Vec3::ONE,
        }
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The integer voxel coordinate containing a world-space position.
///
/// Uses `floor`, not a bare `as i32` cast: truncation rounds toward zero, which
/// would map `-0.5` to voxel `0` instead of the correct `-1`.
pub fn voxel_at(pos: Vec3) -> IVec3 {
    IVec3::new(
        pos.x.floor() as i32,
        pos.y.floor() as i32,
        pos.z.floor() as i32,
    )
}

/// Wrap an angle difference into `[-π, π]` (the shorter way round).
pub fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = a % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    d
}

/// Interpolate from angle `a` toward `b` the shorter way round (remote-player
/// and mob yaw smoothing). The result is not re-wrapped.
pub fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    a + wrap_angle(b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI, TAU};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn wrap_angle_takes_the_shorter_way_round() {
        for (a, want) in [
            (0.0, 0.0),
            (FRAC_PI_2, FRAC_PI_2),
            (-FRAC_PI_2, -FRAC_PI_2),
            (PI + FRAC_PI_2, -FRAC_PI_2),
            (-PI - FRAC_PI_2, FRAC_PI_2),
            (TAU, 0.0),
            (5.0 * TAU + 0.25, 0.25),
            (-7.0 * TAU - 0.25, -0.25),
        ] {
            let got = wrap_angle(a);
            assert!(close(got, want), "wrap_angle({a}) = {got}, want {want}");
            assert!((-PI..=PI).contains(&got));
        }
    }

    #[test]
    fn lerp_angle_crosses_the_seam_instead_of_spinning() {
        // From just below +π to just above -π is a short hop across the seam.
        let (a, b) = (PI - 0.1, -PI + 0.1);
        assert!(close(lerp_angle(a, b, 0.5), PI));
        assert!(close(lerp_angle(a, b, 1.0), PI + 0.1));
        assert!(close(lerp_angle(a, b, 0.0), a));
        assert!(close(lerp_angle(0.0, FRAC_PI_2, 0.5), FRAC_PI_2 / 2.0));
    }

    #[test]
    fn voxel_at_floors_negative_coordinates() {
        assert_eq!(voxel_at(Vec3::new(0.5, 1.999, 2.0)), IVec3::new(0, 1, 2));
        assert_eq!(
            voxel_at(Vec3::new(-0.5, -1.0, -1.001)),
            IVec3::new(-1, -1, -2)
        );
        assert_eq!(voxel_at(Vec3::splat(-0.0)), IVec3::ZERO);
    }

    #[test]
    fn lerp_and_smoothstep_hit_their_endpoints() {
        assert_eq!(lerp(2.0, 6.0, 0.0), 2.0);
        assert_eq!(lerp(2.0, 6.0, 1.0), 6.0);
        assert_eq!(lerp(2.0, 6.0, 0.25), 3.0);
        assert_eq!(smoothstep(1.0, 3.0, 0.0), 0.0);
        assert_eq!(smoothstep(1.0, 3.0, 2.0), 0.5);
        assert_eq!(smoothstep(1.0, 3.0, 9.0), 1.0);
        assert!(smoothstep(1.0, 3.0, 1.5) < 0.25, "eases in");
    }

    #[test]
    fn tilt_eases_to_level_without_overshoot() {
        let t = Tilt::new(0.3, -0.05);
        let eased = t.toward_level(0.1);
        assert!(close(eased.pitch, 0.2) && close(eased.roll, 0.0));
        assert!(t.toward_level(1.0).is_level());
        assert_eq!(Tilt::LEVEL.rotation(), Mat4::IDENTITY);
        assert!(!Tilt::new(f32::NAN, 0.0).is_finite());
    }
}
