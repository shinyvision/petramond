use super::*;

/// A cross-model plant adds a two-plane X billboard to the OPAQUE (cutout) pass,
/// drawn in both windings, and does NOT cull its supporting block's faces.
#[test]
fn cross_plant_emits_double_sided_billboards() {
    // Bare stone cube at an interior voxel: all 6 faces drawn (air neighbours).
    let m0 = mesh(&section_with(&[((8, 8, 8), Block::Stone)]));
    assert_eq!(
        m0.opaque.len(),
        24,
        "interior stone cube should emit 6 quads"
    );

    // Same, plus a short-grass plant on top.
    let m1 = mesh(&section_with(&[
        ((8, 8, 8), Block::Stone),
        ((8, 9, 8), Block::ShortGrass),
    ]));

    // Plant adds 2 planes x 4 verts, each appended a second time in reverse
    // corner order so the plane draws from behind too (the opaque stream's
    // triangulation is implied). The stone's faces are untouched.
    assert_eq!(
        m1.opaque.len() - m0.opaque.len(),
        16,
        "plant should add 2 planes x 2 windings x 4 verts"
    );
    assert!(
        m1.transparent.is_empty(),
        "plant must not feed the alpha pass"
    );
}

/// Leaves must render in the OPAQUE pass, not the alpha-blended one. Proof: a
/// section that has leaves but NO water must produce an empty transparent buffer
/// (only water feeds it now) and a non-empty opaque buffer.
#[test]
fn leaves_go_to_opaque_pass() {
    let m = mesh(&section_with(&[((8, 8, 8), Block::OakLeaves)]));
    assert!(
        m.transparent.is_empty() && m.transparent_two_sided.is_empty(),
        "leaves+no-water section should have an empty transparent buffer"
    );
    assert!(!m.opaque.is_empty(), "leaves should fill the opaque buffer");
}
