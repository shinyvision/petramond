//! Presentation vocabulary shared by the chunk mesher, model instances, and
//! the renderers: the face shade table and the model contact-shadow vertex.

/// Face shade multipliers, index = `Face::shade_idx` (mirrored in the shader).
pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

/// One model contact-shadow vertex: world-space position + darken factor.
/// Keeps blob-shadow identity through fog. 16 bytes, deliberately minimal —
/// the stream is sparse (model cells only).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ContactShadowVertex {
    pub pos: [f32; 3],
    pub darken: f32,
}
