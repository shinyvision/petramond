use super::*;

/// Greedy meshing collapses a flat, uniformly-lit region of identical opaque faces into a
/// single tiled quad, and encodes the merge extent so the shader tiles the tile W×H. A 16×16
/// stone floor's top faces (all AO=3, full sky, same tile+tint) must merge to ONE quad whose
/// packed W,H = 16, and the whole section's opaque geometry must collapse far below the
/// per-cell face count. Pins the merge condition + the (W-1,H-1) packing the shader decodes.
#[test]
fn greedy_merges_flat_floor_into_tiled_quads() {
    let m = mesh(&floor_section(Block::Stone));

    // The 16×16 top (+Y, shade idx 0) at y=1 collapses to a single 4-vertex quad.
    let top: Vec<&Vertex> = m
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 0 && (v.pos[1] - 1.0).abs() < 1e-3)
        .collect();
    assert_eq!(top.len(), 4, "flat 16×16 top should merge into one quad");
    let w = ((top[0].packed >> 12) & 0xF) + 1;
    let h = ((top[0].packed >> 16) & 0xF) + 1;
    assert_eq!(
        (w, h),
        (16, 16),
        "merged top quad must tile its layer 16×16"
    );
    // The quad covers exactly the section footprint. CPU bounds are EXACT: the
    // T-junction crack overlap is a vertex-shader push (`greedy_overlap_push`),
    // never baked here (the packed 1/64 grid cannot hold a sub-pixel offset).
    let (min_x, max_x) = (
        top.iter().map(|v| v.pos[0]).fold(f32::INFINITY, f32::min),
        top.iter()
            .map(|v| v.pos[0])
            .fold(f32::NEG_INFINITY, f32::max),
    );
    assert_eq!((min_x, max_x), (0.0, 16.0));

    // Per cell this floor would emit 256 top + 256 bottom + 4×16 side faces = 576 quads
    // (2304 verts); greedy collapses it to a handful.
    assert!(
        m.opaque.len() < 64,
        "greedy should collapse the flat floor, got {} verts",
        m.opaque.len()
    );
}
