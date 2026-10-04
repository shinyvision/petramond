use super::*;

#[test]
fn a_pinned_end_holds_while_the_free_end_takes_the_whole_correction() {
    let mut a = Vec3::ZERO;
    let mut b = Vec3::new(2.0, 0.0, 0.0);
    satisfy_distance(&mut a, &mut b, 1.0, 0.0, 1.0, 1.0);
    assert_eq!(a, Vec3::ZERO);
    assert!((b.x - 1.0).abs() < 1e-5);
}

#[test]
fn a_fast_point_stops_on_a_thin_floor_instead_of_tunnelling() {
    let floor = |c: IVec3| c.y == 0;
    let out = resolve_through_cells(Vec3::new(0.5, 5.5, 0.5), Vec3::new(0.5, -6.0, 0.5), &floor);
    assert!(out.y > 1.0 && out.y < 1.01, "{out}");
}

#[test]
fn capsule_push_out_is_radial_so_cloth_flows_around_a_body() {
    let p = Vec3::new(0.2, 1.0, 0.0);
    let out = push_out_of_capsule(p, Vec3::ZERO, Vec3::Y * 2.0, 0.4).unwrap();
    assert!((out - Vec3::new(0.4, 1.0, 0.0)).length() < 1e-5, "{out}");
    assert!(
        push_out_of_capsule(Vec3::new(1.0, 1.0, 0.0), Vec3::ZERO, Vec3::Y * 2.0, 0.4).is_none()
    );
}

#[test]
fn a_box_edge_piercing_a_triangle_between_its_vertices_is_zero_distance() {
    // Every vertex sits outside the unit box, but its top edge runs through the sheet:
    // the case vertex-only collision misses.
    let (a, b, c) = (
        Vec3::new(-0.5, 1.2, 0.5),
        Vec3::new(1.5, 1.2, 0.5),
        Vec3::new(0.5, 0.3, 2.0),
    );
    assert!(closest::triangle_overlaps_box(
        a,
        b,
        c,
        Vec3::ZERO,
        Vec3::ONE
    ));
    assert_eq!(
        closest::triangle_box_distance(a, b, c, Vec3::ZERO, Vec3::ONE),
        0.0
    );
}

#[test]
fn triangle_box_distance_is_reached_at_an_edge_edge_pair() {
    // A sheet tilted over the box's top edge: nearest at the box edge, not a vertex.
    let (a, b, c) = (
        Vec3::new(-1.0, 1.0, 1.4),
        Vec3::new(2.0, 1.0, 1.4),
        Vec3::new(0.5, 2.4, 0.0),
    );
    let d = closest::triangle_box_distance(a, b, c, Vec3::ZERO, Vec3::ONE);
    let along_edge =
        closest::closest_segments(a, b, Vec3::new(0.0, 1.0, 1.0), Vec3::new(1.0, 1.0, 1.0));
    assert!(d > 0.0 && d < 0.4 + 1e-4, "{d}");
    assert!((along_edge.2 - along_edge.3).length() >= d - 1e-5);
}
