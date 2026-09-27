use crate::face::Face;

pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

pub trait FaceShading: Sized {
    fn shade_idx(self) -> u32;

    fn normal_code(self) -> u32;

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
