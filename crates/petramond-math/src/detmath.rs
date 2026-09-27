use glam::{Mat4, Quat, Vec4};

#[inline]
pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}

#[inline]
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}

#[inline]
pub fn sin_cos(x: f64) -> (f64, f64) {
    (libm::sin(x), libm::cos(x))
}

#[inline]
pub fn exp(x: f64) -> f64 {
    libm::exp(x)
}

#[inline]
pub fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

#[inline]
pub fn sinf(x: f32) -> f32 {
    libm::sinf(x)
}

#[inline]
pub fn cosf(x: f32) -> f32 {
    libm::cosf(x)
}

#[inline]
pub fn sin_cosf(x: f32) -> (f32, f32) {
    (libm::sinf(x), libm::cosf(x))
}

#[inline]
pub fn expf(x: f32) -> f32 {
    libm::expf(x)
}

#[inline]
pub fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}

#[inline]
pub fn cbrtf(x: f32) -> f32 {
    libm::cbrtf(x)
}

#[inline]
pub fn quat_rotation_x(angle: f32) -> Quat {
    let (s, c) = sin_cosf(angle * 0.5);
    Quat::from_xyzw(s, 0.0, 0.0, c)
}

#[inline]
pub fn quat_rotation_y(angle: f32) -> Quat {
    let (s, c) = sin_cosf(angle * 0.5);
    Quat::from_xyzw(0.0, s, 0.0, c)
}

#[inline]
pub fn quat_rotation_z(angle: f32) -> Quat {
    let (s, c) = sin_cosf(angle * 0.5);
    Quat::from_xyzw(0.0, 0.0, s, c)
}

pub const QUARTER_TURN_Y: Quat = Quat::from_xyzw(
    0.0,
    -std::f32::consts::FRAC_1_SQRT_2,
    0.0,
    std::f32::consts::FRAC_1_SQRT_2,
);

#[inline]
pub fn mat4_rotation_x(angle: f32) -> Mat4 {
    let (s, c) = sin_cosf(angle);
    Mat4::from_cols(
        Vec4::X,
        Vec4::new(0.0, c, s, 0.0),
        Vec4::new(0.0, -s, c, 0.0),
        Vec4::W,
    )
}

#[inline]
pub fn mat4_rotation_y(angle: f32) -> Mat4 {
    let (s, c) = sin_cosf(angle);
    Mat4::from_cols(
        Vec4::new(c, 0.0, -s, 0.0),
        Vec4::Y,
        Vec4::new(s, 0.0, c, 0.0),
        Vec4::W,
    )
}

#[inline]
pub fn mat4_rotation_z(angle: f32) -> Mat4 {
    let (s, c) = sin_cosf(angle);
    Mat4::from_cols(
        Vec4::new(c, s, 0.0, 0.0),
        Vec4::new(-s, c, 0.0, 0.0),
        Vec4::Z,
        Vec4::W,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_exact_where_exact_and_close_elsewhere() {
        assert_eq!(pow(3.0, 4.0), 81.0);
        assert_eq!(pow(2.0, -2.0), 0.25);
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(cbrtf(27.0), 3.0);
        assert_eq!(expf(0.0), 1.0);
        assert_eq!(powf(2.0, 10.0), 1024.0);
        assert_eq!(sin_cos(0.25), (sin(0.25), cos(0.25)));
        assert_eq!(sin_cosf(0.25), (sinf(0.25), cosf(0.25)));
        let ulps = |a: f64, b: f64| (a.to_bits() as i64 - b.to_bits() as i64).unsigned_abs();
        let ulpsf = |a: f32, b: f32| (a.to_bits() as i32 - b.to_bits() as i32).unsigned_abs();
        for i in 1..200 {
            let x = f64::from(i) * 0.137;
            assert!(
                ulps(sin(x), x.sin()) <= 2 || sin(x).abs() < 1e-3,
                "sin({x})"
            );
            assert!(
                ulps(cos(x), x.cos()) <= 2 || cos(x).abs() < 1e-3,
                "cos({x})"
            );
            assert!(ulps(exp(-x), (-x).exp()) <= 2, "exp(-{x})");
            assert!(ulps(pow(x, 0.85), x.powf(0.85)) <= 2, "pow({x}, 0.85)");
            let xf = x as f32;
            assert!(
                ulpsf(sinf(xf), xf.sin()) <= 2 || sinf(xf).abs() < 1e-3,
                "sinf({xf})"
            );
            assert!(ulpsf(powf(xf, 1.7), xf.powf(1.7)) <= 2, "powf({xf}, 1.7)");
            assert!(ulpsf(cbrtf(xf), xf.cbrt()) <= 2, "cbrtf({xf})");
        }
    }

    #[test]
    fn rotations_match_glam_layouts() {
        let close = |a: Mat4, b: Mat4| a.abs_diff_eq(b, 1e-6);
        for angle in [0.3_f32, -1.2, 2.5] {
            assert!(close(mat4_rotation_x(angle), Mat4::from_rotation_x(angle)));
            assert!(close(mat4_rotation_y(angle), Mat4::from_rotation_y(angle)));
            assert!(close(mat4_rotation_z(angle), Mat4::from_rotation_z(angle)));
            assert!(quat_rotation_x(angle).abs_diff_eq(Quat::from_rotation_x(angle), 1e-6));
            assert!(quat_rotation_y(angle).abs_diff_eq(Quat::from_rotation_y(angle), 1e-6));
            assert!(quat_rotation_z(angle).abs_diff_eq(Quat::from_rotation_z(angle), 1e-6));
        }
        assert_eq!(mat4_rotation_x(0.0), Mat4::IDENTITY);
        let turned = QUARTER_TURN_Y * glam::Vec3::X;
        assert!(turned.abs_diff_eq(glam::Vec3::Z, 1e-6), "{turned:?}");
        let reference = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        assert!(QUARTER_TURN_Y.abs_diff_eq(reference, 1e-7));
    }
}
