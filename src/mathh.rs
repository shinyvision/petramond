//! Misc math helpers not covered by glam.

pub use glam::{IVec2, IVec3, Mat4, Quat, Vec2, Vec3, Vec4};

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SelectionShape {
    Box {
        min: Vec3,
        max: Vec3,
    },
    Cross {
        origin: IVec3,
        u_min: f32,
        u_max: f32,
        v_min: f32,
        v_max: f32,
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

pub fn smoothstep01(x: f32) -> f32 {
    smoothstep(0.0, 1.0, x)
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
