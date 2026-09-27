use super::*;

/// A flat 16x16 stone floor's top should come out as one quad, W and H both 16, instead of 256. We
/// decode (W-1,H-1) in the shader, so the packing is pinned here too.
#[test]
fn greedy_merges_flat_floor_into_tiled_quads() {
    let m = mesh(&floor_section(Block::Stone));

    let top: Vec<&Vertex> = m
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 0 && (v.pos[1] - 1.0).abs() < 1e-3)
        .collect();
    assert_eq!(top.len(), 4, "flat 16×16 top should merge into one quad");
    assert_eq!(
        crate::vertex::unpack_greedy_span(top[0].packed2),
        (16, 16),
        "merged top quad must tile its layer 16×16"
    );
    let (min_x, max_x) = (
        top.iter().map(|v| v.pos[0]).fold(f32::INFINITY, f32::min),
        top.iter()
            .map(|v| v.pos[0])
            .fold(f32::NEG_INFINITY, f32::max),
    );
    assert_eq!((min_x, max_x), (0.0, 16.0));

    assert!(
        m.opaque.len() < 64,
        "greedy should collapse the flat floor, got {} verts",
        m.opaque.len()
    );
}
