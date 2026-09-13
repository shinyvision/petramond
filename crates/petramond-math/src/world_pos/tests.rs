use super::*;

#[test]
fn relative_offsets_stay_exact_far_from_the_origin() {
    let far = 1_000_000_000;
    let anchor = IVec3::new(far, 64, -far);
    let pos = WorldPos::new(f64::from(far) + 3.015625, 70.25, f64::from(-far) - 0.5);
    assert_eq!(pos.relative_to(anchor), Vec3::new(3.015625, 6.25, -0.5));
    assert_eq!(pos.block(), IVec3::new(far + 3, 70, -far - 1));

    let stepped = pos + Vec3::new(0.0009765625, 0.0, -0.0009765625);
    assert_eq!(stepped - pos, Vec3::new(0.0009765625, 0.0, -0.0009765625));
}
