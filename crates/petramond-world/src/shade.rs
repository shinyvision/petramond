//! Presentation vocabulary shared by the chunk mesher, model instances, and
//! the renderers: the face shade table, the per-face shade and normal codes
//! packed into vertices, and the model contact-shadow vertex.
//!
//! It lives here rather than in `petramond-mesh` because model instances bake
//! their cell templates (shade included) in this crate, below the mesher.

use crate::face::Face;

/// Face shade multipliers, index = [`FaceShading::shade_idx`] (mirrored in the
/// shader).
pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

/// How a [`Face`] is shaded and encoded in the packed vertex formats: the
/// terrain shader's codes, kept out of the geometry-only `Face` primitive.
pub trait FaceShading: Sized {
    /// Index into [`SHADES`] (and the shader's mirror) — packed into the
    /// vertex instead of the raw float.
    fn shade_idx(self) -> u32;

    /// Face-normal code for the packed vertex's normal lane (see
    /// `petramond_mesh::vertex::pack_normal_code`): 1..=6 in `Face::ALL`
    /// order. Code 0 is reserved for "neutral" geometry with no meaningful
    /// world-space face direction (cross plants, torches, dynamic props) —
    /// the shader falls back to the classic [`SHADES`] table for it instead
    /// of sun N·L shading.
    fn normal_code(self) -> u32;

    /// The face's [`SHADES`] multiplier.
    fn shade(self) -> f32 {
        SHADES[self.shade_idx() as usize]
    }
}

impl FaceShading for Face {
    #[inline]
    fn shade_idx(self) -> u32 {
        match self {
            Face::PosY => 0,
            Face::PosZ | Face::NegZ => 1,
            Face::PosX | Face::NegX => 2,
            Face::NegY => 3,
        }
    }

    #[inline]
    fn normal_code(self) -> u32 {
        self as u32 + 1
    }
}

/// One model contact-shadow vertex: mesh-space position (column-local XZ,
/// world Y) + darken factor.
/// Keeps blob-shadow identity through fog. 16 bytes, deliberately minimal —
/// the stream is sparse (model cells only).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ContactShadowVertex {
    pub pos: [f32; 3],
    pub darken: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_codes_follow_face_order_and_reserve_zero() {
        for (i, face) in Face::ALL.into_iter().enumerate() {
            assert_eq!(face.normal_code(), i as u32 + 1);
        }
    }

    #[test]
    fn shading_is_top_brightest_bottom_darkest_and_axis_symmetric() {
        assert_eq!(Face::PosY.shade(), SHADES[0]);
        assert_eq!(Face::NegY.shade(), SHADES[3]);
        for pair in Face::ALL.chunks(2) {
            if pair[0] != Face::PosY {
                assert_eq!(pair[0].shade_idx(), pair[1].shade_idx(), "{pair:?}");
            }
        }
        assert!(SHADES.windows(2).all(|w| w[0] > w[1]));
    }
}
