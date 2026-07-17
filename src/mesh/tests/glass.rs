use super::*;

/// Faces between two adjacent glass blocks are invisible (you'd see a frame
/// floating inside the glass), so the mesher culls them BOTH ways — a glass
/// wall reads as one sheet. Glass↔air and glass↔stone faces still draw.
#[test]
fn adjacent_glass_blocks_cull_their_shared_faces() {
    let m_solo = mesh(&section_with(&[((8, 8, 8), Block::Glass)]));
    assert_eq!(m_solo.opaque.len(), 24, "lone glass emits all 6 faces");

    let m_pair = mesh(&section_with(&[
        ((8, 8, 8), Block::Glass),
        ((9, 8, 8), Block::Glass),
    ]));
    assert_eq!(
        m_pair.opaque.len(),
        40,
        "a glass pair shares one culled face pair: 12 - 2 = 10 faces"
    );
}

/// A lone pane is a bare post (4 edge sides + 2 caps). Two adjacent panes grow
/// arms toward each other and bury the end faces at the shared cell boundary,
/// so the run reads as one continuous sheet of glass. A pane beside a
/// `no_pane_connect` block (the inset cactus) stays a bare post.
#[test]
fn pane_arms_connect_and_bury_shared_end_faces() {
    let m_lone = mesh(&section_with(&[((8, 8, 8), Block::GlassPane)]));
    assert_eq!(m_lone.opaque.len(), 24, "bare post: 4 sides + 2 caps");

    // Two connected panes: per pane an east/west run (2 broad faces + 1 free-end
    // edge strip) + post and arm caps (4) = 7 quads; nothing on the shared plane.
    let m_pair = mesh(&section_with(&[
        ((8, 8, 8), Block::GlassPane),
        ((9, 8, 8), Block::GlassPane),
    ]));
    assert_eq!(
        m_pair.opaque.len(),
        56,
        "connected pair: 7 quads per pane, no faces at the shared boundary"
    );
    let boundary = 9.0f32;
    assert!(
        !m_pair
            .opaque
            .iter()
            .all(|v| (v.pos[0] - boundary).abs() < f32::EPSILON),
        "sanity: vertices exist off the boundary plane"
    );
    let quad_on_boundary = m_pair
        .opaque
        .chunks(4)
        .any(|q| q.iter().all(|v| (v.pos[0] - boundary).abs() < f32::EPSILON));
    assert!(
        !quad_on_boundary,
        "no quad may lie in the shared cell-boundary plane"
    );

    // The cactus carries no_pane_connect: the pane beside it stays a bare post.
    // Every pane vertex lives in the post's thin span (x < 9), every cactus
    // vertex at x >= 9, so the pane's share of the mesh is cleanly separable.
    let m = mesh(&section_with(&[
        ((8, 8, 8), Block::GlassPane),
        ((9, 8, 8), Block::Cactus),
    ]));
    let pane_verts = m.opaque.iter().filter(|v| v.pos[0] < 9.0).count();
    assert_eq!(
        pane_verts, 24,
        "pane beside a cactus keeps the bare-post shape"
    );
}
