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

#[test]
fn distant_canopy_keeps_exterior_sprays_and_materials() {
    // BOTH emitters: cube-family leaves take the exposure-mask fast path when a
    // pad is present and the generic per-face path when it is not, and each has
    // to split the leaf-to-leaf internals off the far LOD's prefix for itself.
    type Mesher = fn(&Section) -> ChunkMesh;
    for (path, build) in [
        ("closures", mesh as Mesher),
        ("pad", mesh_via_pad as Mesher),
    ] {
        for leaf in [Block::OakLeaves, Block::SpruceLeaves] {
            let mut section = Section::new(0, 0, 0);
            for x in 6..10 {
                for y in 6..10 {
                    for z in 6..10 {
                        section.set_block(x, y, z, leaf);
                    }
                }
            }
            let near = build(&section);
            assert!(
                near.far_opaque_len > 0 && near.far_opaque_len < near.opaque.len() as u32,
                "{path}: a solid leaf cube has internal faces for the far LOD to drop"
            );
            let far_opaque = &near.opaque[..near.far_opaque_len as usize];
            let near_quads: std::collections::HashSet<Vec<u8>> = near
                .opaque
                .chunks_exact(4)
                .map(|q| bytemuck::cast_slice::<Vertex, u8>(q).to_vec())
                .collect();
            for q in far_opaque.chunks_exact(4) {
                assert!(
                    near_quads.contains(bytemuck::cast_slice::<Vertex, u8>(q)),
                    "distant foliage must preserve exterior shape, UVs and lighting"
                );
            }
            let is_spray = |q: &[Vertex]| {
                uv_mode(&q[0]) == crate::vertex::UV_MODE_NONE
                    && q.iter()
                        .any(|v| v.pos.iter().any(|&p| !(6.0..=10.0).contains(&p)))
            };
            let sprays: Vec<_> = near
                .opaque
                .chunks_exact(4)
                .filter(|q| is_spray(q))
                .collect();
            assert!(!sprays.is_empty());
            let far_sprays: Vec<_> = far_opaque.chunks_exact(4).filter(|q| is_spray(q)).collect();
            assert_eq!(sprays.len(), far_sprays.len());
            for (near, far) in sprays.iter().zip(far_sprays) {
                assert_eq!(
                    bytemuck::cast_slice::<Vertex, u8>(near),
                    bytemuck::cast_slice::<Vertex, u8>(far)
                );
            }
            for q in sprays {
                assert!(
                    q.iter()
                        .any(|v| v.pos.iter().any(|&p| !(6.0..=10.0).contains(&p))),
                    "sprays belong on the outside of the crown"
                );
            }
        }
    }
}

#[test]
fn leaf_sprays_do_not_enter_occupied_or_unloaded_neighbors() {
    let mut blocks = vec![((8, 8, 8), Block::OakLeaves)];
    for (x, y, z) in [
        (7, 8, 8),
        (9, 8, 8),
        (8, 7, 8),
        (8, 9, 8),
        (8, 8, 7),
        (8, 8, 9),
    ] {
        blocks.push(((x, y, z), Block::Glass));
    }
    let surrounded = mesh(&section_with(&blocks));
    assert!(surrounded
        .opaque
        .iter()
        .all(|v| v.pos.iter().all(|p| p.fract() == 0.0)));
    let unloaded = mesh_with(
        &section_with(&[((8, 8, 8), Block::OakLeaves)]),
        |_, _, _| SKY_FULL,
        |_, _, _| false,
    );
    assert!(unloaded
        .opaque
        .iter()
        .all(|v| v.pos.iter().all(|p| p.fract() == 0.0)));
}
