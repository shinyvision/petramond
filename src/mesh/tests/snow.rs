use super::*;

/// A lowered cube's full 1×1 base is flush with the cell floor, so the full
/// block beneath it must CULL its top face — the two nearly-coplanar planes
/// (carrier top at y, snow top at y+1/16) z-fight from far above otherwise.
/// The lowered cube itself keeps rendering (its sunken top is inside the cell).
#[test]
fn a_full_cube_under_a_snow_layer_culls_its_top_face() {
    let mut section = floor_section(Block::Stone);
    section.set_block(8, 1, 8, Block::SnowLayer);
    let m = mesh(&section);

    // Every opaque emitter here pushes 4-vertex quads; group and classify.
    let quads: Vec<&[Vertex]> = m.opaque.chunks_exact(4).collect();
    let covers = |q: &[Vertex], x: f32, z: f32| {
        let (mut xmin, mut xmax, mut zmin, mut zmax) =
            (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
        for v in q {
            xmin = xmin.min(v.pos[0]);
            xmax = xmax.max(v.pos[0]);
            zmin = zmin.min(v.pos[2]);
            zmax = zmax.max(v.pos[2]);
        }
        xmin < x && x < xmax && zmin < z && z < zmax
    };

    // No floor-top quad (y=1, +Y shade) may cover the snow-carrying cell.
    let carrier_top_covered = quads.iter().any(|q| {
        shade_idx(&q[0]) == 0
            && q.iter().all(|v| (v.pos[1] - 1.0).abs() < 1e-3)
            && covers(q, 8.5, 8.5)
    });
    assert!(
        !carrier_top_covered,
        "the block under a snow layer must not emit its covered top face"
    );

    // The snow layer's own sunken top (y = 1 + 1/16) still renders.
    let snow_top = quads.iter().any(|q| {
        shade_idx(&q[0]) == 0
            && q.iter().all(|v| (v.pos[1] - (1.0 + 1.0 / 16.0)).abs() < 1e-3)
            && covers(q, 8.5, 8.5)
    });
    assert!(snow_top, "the snow layer's own top face must keep rendering");

    // An uncovered floor cell still has its top drawn (the cull is per cell).
    let open_top_covered = quads.iter().any(|q| {
        shade_idx(&q[0]) == 0
            && q.iter().all(|v| (v.pos[1] - 1.0).abs() < 1e-3)
            && covers(q, 2.5, 2.5)
    });
    assert!(open_top_covered, "uncovered floor tops must still render");
}
