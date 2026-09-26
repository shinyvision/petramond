use super::*;

/// The size gate is a DISTANCE derived from a pixel scale, and the direction of
/// that derivation is the easy thing to invert: a reciprocal in the wrong place
/// culls everything near instead of everything far, and both mistakes look like
/// a working game until someone counts particles.
#[test]
fn detail_is_kept_while_it_covers_a_pixel_and_dropped_once_it_cannot() {
    // 1080 rows over a 70° vertical fov: a block spans ~771 px one block out.
    let scale = 0.5 * 1080.0 / 35.0_f32.to_radians().tan();
    let view = ViewVolume::new(
        Frustum::permissive(),
        IVec3::ZERO,
        WorldPos::ZERO,
        f32::INFINITY,
        scale,
    );
    let cell_at = |d: f64| (WorldPos::new(d, 0.0, 0.0), WorldPos::new(d + 1.0, 1.0, 1.0));

    // A 7 cm ember: visible close by, gone long before the render distance.
    let (lo, hi) = cell_at(10.0);
    assert!(view.covers_a_pixel(lo, hi, 0.07));
    let (lo, hi) = cell_at(200.0);
    assert!(!view.covers_a_pixel(lo, hi, 0.07));

    // Anything cell-sized stays visible across the whole volume — the gate is
    // for SUB-CELL detail and must never reach ordinary geometry.
    let (lo, hi) = cell_at(600.0);
    assert!(view.covers_a_pixel(lo, hi, 1.0));

    // No camera, no gate.
    assert!(ViewVolume::unbounded().covers_a_pixel(lo, hi, 0.001));
}
