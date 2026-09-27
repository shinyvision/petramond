use super::*;

#[test]
fn fence_rails_connect_and_never_show_end_faces() {
    let m_lone = mesh(&section_with(&[((8, 8, 8), Block::OakFence)]));
    assert_eq!(m_lone.opaque.len(), 24, "bare post: 4 sides + 2 caps");

    let m_pair = mesh(&section_with(&[
        ((8, 8, 8), Block::OakFence),
        ((9, 8, 8), Block::OakFence),
    ]));
    assert_eq!(
        m_pair.opaque.len(),
        160,
        "connected pair: 20 quads per fence, no rail end caps"
    );
    let boundary = 9.0f32;
    let quad_on_boundary = m_pair
        .opaque
        .chunks(4)
        .any(|q| q.iter().all(|v| (v.pos[0] - boundary).abs() < f32::EPSILON));
    assert!(
        !quad_on_boundary,
        "no quad may lie in the shared cell-boundary plane"
    );

    let m = mesh(&section_with(&[
        ((8, 8, 8), Block::OakFence),
        ((9, 8, 8), Block::OakLeaves),
    ]));
    let fence_verts = m.opaque.iter().filter(|v| v.pos[0] < 9.0).count();
    assert_eq!(fence_verts, 24, "fence beside leaves stays a bare post");
}

#[test]
fn stacked_fences_bury_the_shared_post_cap() {
    let m = mesh(&section_with(&[
        ((8, 8, 8), Block::OakFence),
        ((8, 9, 8), Block::OakFence),
    ]));
    assert_eq!(
        m.opaque.len(),
        40,
        "two posts' sides (8 quads) + the two exposed outer caps"
    );
    let seam = 9.0f32;
    let cap_on_seam = m
        .opaque
        .chunks(4)
        .any(|q| q.iter().all(|v| (v.pos[1] - seam).abs() < f32::EPSILON));
    assert!(
        !cap_on_seam,
        "no cap may lie on the shared horizontal plane"
    );
}
