//! Misc math helpers not covered by glam.

pub use glam::{IVec3, Mat4, Vec3, Vec4};

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

    /// The rotation inside the yaw: `Rx(pitch) · Rz(roll)`.
    pub fn rotation(self) -> Mat4 {
        Mat4::from_rotation_x(self.pitch) * Mat4::from_rotation_z(self.roll)
    }

    /// The whole body frame for a mob-convention `yaw` (`0` faces `-Z`):
    /// `Ry(yaw) · Rx(pitch) · Rz(roll)`.
    pub fn body_frame(self, yaw: f32) -> Mat4 {
        Mat4::from_rotation_y(yaw) * self.rotation()
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

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SelectionShape {
    Box {
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
    /// A shape made from a small fixed list of world-space boxes. Used for stairs so
    /// the outline traces the solid stair volume instead of a full block cube.
    Boxes {
        boxes: SelectionBoxes,
    },
}

impl SelectionShape {
    pub fn full_block(block: IVec3) -> Self {
        Self::Box {
            min: Vec3::new(block.x as f32, block.y as f32, block.z as f32),
            max: Vec3::new(
                block.x as f32 + 1.0,
                block.y as f32 + 1.0,
                block.z as f32 + 1.0,
            ),
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

/// Wrap an angle difference into `(-π, π]`.
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

/// remote-player interpolation.
pub fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    a + wrap_angle(b - a) * t
}
