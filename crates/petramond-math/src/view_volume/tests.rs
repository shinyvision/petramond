use super::*;

#[test]
fn detail_is_kept_while_it_covers_a_pixel_and_dropped_once_it_cannot() {
    let scale = 0.5 * 1080.0 / 35.0_f32.to_radians().tan();
    let view = ViewVolume::new(
        Frustum::permissive(),
        IVec3::ZERO,
        WorldPos::ZERO,
        f32::INFINITY,
        scale,
    );
    let cell_at = |d: f64| (WorldPos::new(d, 0.0, 0.0), WorldPos::new(d + 1.0, 1.0, 1.0));

    let (lo, hi) = cell_at(10.0);
    assert!(view.covers_a_pixel(lo, hi, 0.07));
    let (lo, hi) = cell_at(200.0);
    assert!(!view.covers_a_pixel(lo, hi, 0.07));

    let (lo, hi) = cell_at(600.0);
    assert!(view.covers_a_pixel(lo, hi, 1.0));

    assert!(ViewVolume::unbounded().covers_a_pixel(lo, hi, 0.001));
}
