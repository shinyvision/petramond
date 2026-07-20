use super::*;

/// A lone fence is a bare post (4 sides + 2 caps). Two adjacent fences grow
/// rail pairs toward each other with no end faces at the shared cell boundary,
/// so the run reads as continuous rails between the posts. A fence beside a
/// transparent block (leaves) stays a bare post.
#[test]
fn fence_rails_connect_and_never_show_end_faces() {
    let m_lone = mesh(&section_with(&[((8, 8, 8), Block::OakFence)]));
    assert_eq!(m_lone.opaque.len(), 24, "bare post: 4 sides + 2 caps");

    // Two connected fences: each grows ONE rail pair toward the other, so per
    // fence the post (6 quads) + one rail pair (2 rails × 4 long faces) = 14
    // quads; the rails carry no end faces, so nothing lies on the shared
    // cell-boundary plane.
    let m_pair = mesh(&section_with(&[
        ((8, 8, 8), Block::OakFence),
        ((9, 8, 8), Block::OakFence),
    ]));
    assert_eq!(
        m_pair.opaque.len(),
        112,
        "connected pair: 14 quads per fence, no rail end caps"
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

    // Leaves are transparent: the fence beside them keeps the bare-post shape
    // (every fence vertex lives in the post's span, x < 9).
    let m = mesh(&section_with(&[
        ((8, 8, 8), Block::OakFence),
        ((9, 8, 8), Block::OakLeaves),
    ]));
    let fence_verts = m.opaque.iter().filter(|v| v.pos[0] < 9.0).count();
    assert_eq!(fence_verts, 24, "fence beside leaves stays a bare post");
}

/// Stacked fences hide the shared post cap both ways; the outer caps stay.
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
    assert!(!cap_on_seam, "no cap may lie on the shared horizontal plane");
}
