use super::*;

#[test]
fn stair_bottom_face_uses_the_dark_cell_below_not_smooth_sky_leak() {
    let mut section = section_with(&[((8, 8, 8), Block::OakStairs)]);
    section.set_stair_facing(8, 8, 8, Facing::East);

    // The layer below is fully sky-lit EXCEPT the cell directly under the stair.
    let m = mesh_with_sky(&section, |wx, wy, wz| {
        if wy == 7 && (wx, wz) != (8, 8) {
            SKY_FULL
        } else {
            0
        }
    });

    let bottom = m
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 3 && (v.pos[1] - 8.0).abs() < 1.0e-3)
        .collect::<Vec<_>>();
    assert!(!bottom.is_empty(), "stair should emit bottom-face vertices");
    assert!(
        bottom.iter().all(|v| light6(v) == 0),
        "a stair's solid bottom must not show skylight from adjacent below cells"
    );
}

/// Every stair face carries cell-local UVs, and the exposed full bottom plane
/// merges into ONE quad spanning one full tile — the underside must read as an
/// uncut block face, not four restarted quadrants.
#[test]
fn stair_underside_is_one_full_tile_quad_with_cell_local_uvs() {
    use crate::mesh::vertex::UV_MODE_CELL_LOCAL;

    let m = mesh_stairs(
        &[((8, 8, 8), Block::RedwoodStairs)],
        &[((8, 8, 8), Facing::South)],
    );

    assert!(!m.opaque.is_empty());
    for v in &m.opaque {
        assert_eq!(
            uv_mode(v),
            UV_MODE_CELL_LOCAL,
            "every stair vertex must carry explicit cell-local UVs"
        );
    }

    let bottom: Vec<_> = m
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 3 && (v.pos[1] - 8.0).abs() < 1.0e-3)
        .collect();
    assert_eq!(
        bottom.len(),
        4,
        "the full underside plane must merge into a single quad"
    );
    let mut uvs: Vec<(u32, u32)> = bottom.iter().map(|v| cell_uv16(v)).collect();
    uvs.sort_unstable();
    assert_eq!(
        uvs,
        vec![(0, 0), (0, 16), (16, 0), (16, 16)],
        "the underside quad must span exactly one full tile"
    );
}

/// The screenshot regression: a stair beside a wall must shade as one smooth
/// gradient per plane. Same-position vertices of the same face kind must carry
/// identical lighting and UVs (no seams inside a plane, including the L-shaped
/// side faces), the tread top must be a single seam-free quad, and both top
/// planes must darken toward the wall (the AO gradient survives, continuously).
#[test]
fn stair_plane_lighting_is_continuous_beside_a_wall() {
    let m = mesh_stairs(
        &[
            ((8, 8, 8), Block::OakStairs),
            ((7, 8, 8), Block::Stone),
            ((7, 9, 8), Block::Stone),
        ],
        &[((8, 8, 8), Facing::South)],
    );

    // Continuity: group the STAIR's own vertices (cell-local UVs — the wall's
    // plain cube faces share exact corner positions with the stair box and are
    // legitimately a different tile/format) by (position, face kind); every
    // group must agree on both packed words (AO + sky light + block light +
    // cell UV). Face kind is shade index plus the normal code, so coincident
    // corners of differently-facing sub-quads never compare.
    let mut seen: std::collections::HashMap<([u32; 3], u32, u32), (u32, u32)> =
        std::collections::HashMap::new();
    for v in &m.opaque {
        if uv_mode(v) != UV_MODE_CELL_LOCAL {
            continue;
        }
        let key = (v.pos.map(f32::to_bits), shade_idx(v), (v.packed2 >> 16) & 0x7);
        // Ignore the per-quad corner index (bits 8..10); everything else —
        // tile, AO, sky light, block light, cell UV — must agree.
        let val = (v.packed & !(0x3 << 8), v.packed2);
        if let Some(prev) = seen.insert(key, val) {
            assert_eq!(
                prev, val,
                "coplanar stair vertices at {:?} must shade and texture identically",
                v.pos
            );
        }
    }

    let tread: Vec<_> = m
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 0 && (v.pos[1] - 8.5).abs() < 1.0e-3)
        .collect();
    assert_eq!(
        tread.len(),
        4,
        "the tread top must merge into a single quad (no mid-face seam)"
    );

    // Corner-for-corner at equal z (the riser crease's self-AO darkens BOTH
    // x ends of the crease equally, so compare like corners, not plane-wide
    // extremes): the wall side is never brighter, and strictly darker
    // somewhere on each top plane.
    for plane_y in [8.5, 9.0] {
        let plane: Vec<_> = m
            .opaque
            .iter()
            .filter(|v| shade_idx(v) == 0 && (v.pos[1] - plane_y).abs() < 1.0e-3)
            .collect();
        let mut strictly_darker = false;
        for v in plane.iter().filter(|v| (v.pos[0] - 8.0).abs() < 1.0e-3) {
            let partner = plane
                .iter()
                .find(|w| {
                    (w.pos[0] - 9.0).abs() < 1.0e-3 && (w.pos[2] - v.pos[2]).abs() < 1.0e-3
                })
                .expect("matching open-side corner");
            assert!(
                ao_idx(v) <= ao_idx(partner),
                "top plane at y {plane_y}: wall side must never be brighter"
            );
            strictly_darker |= ao_idx(v) < ao_idx(partner);
        }
        assert!(
            strictly_darker,
            "top plane at y {plane_y} must darken toward the wall"
        );
    }
}

/// A stair is a full block with a chunk cut out: a face plane the cut does not
/// touch (the underside) must shade corner-for-corner like the same face of a
/// full cube in an identical neighbourhood.
#[test]
fn stair_underside_shades_like_a_full_block_bottom() {
    let m = mesh_stairs(
        &[
            ((8, 8, 8), Block::OakStairs),
            ((7, 7, 8), Block::Stone),
            ((12, 8, 8), Block::Stone),
            ((11, 7, 8), Block::Stone),
        ],
        &[((8, 8, 8), Facing::East)],
    );

    let ao_at = |x: f32, z: f32| ao_idx(vert_at(&m.opaque, 3, [x, 8.0, z]));

    for dz in [0.0, 1.0] {
        for (stair_x, cube_x) in [(8.0, 12.0), (9.0, 13.0)] {
            assert_eq!(
                ao_at(stair_x, 8.0 + dz),
                ao_at(cube_x, 8.0 + dz),
                "stair underside corner ({stair_x}, {dz}) must match the full cube's"
            );
        }
    }
    assert!(
        ao_at(8.0, 8.0) < ao_at(9.0, 8.0),
        "the occluder must actually differentiate the corners this test compares"
    );
}

#[test]
fn stair_mesh_uses_resolved_outside_corner_shape() {
    let m = mesh_stairs(
        &[((8, 8, 8), Block::OakStairs), ((7, 8, 8), Block::OakStairs)],
        &[((8, 8, 8), Facing::East), ((7, 8, 8), Facing::South)],
    );

    let target_high_top = m
        .opaque
        .iter()
        .filter(|v| {
            (v.pos[1] - 9.0).abs() < 1.0e-3
                && v.pos[0] >= 8.0 - 1.0e-3
                && v.pos[0] < 9.0 - 1.0e-3
                && v.pos[2] >= 8.0 - 1.0e-3
                && v.pos[2] < 9.0 - 1.0e-3
        })
        .collect::<Vec<_>>();

    assert!(
        !target_high_top.is_empty(),
        "target stair should still have one high quadrant"
    );
    assert!(
        target_high_top
            .iter()
            .all(|v| v.pos[0] <= 8.5 + 1.0e-3 && v.pos[2] <= 8.5 + 1.0e-3),
        "the high-side perpendicular neighbour must render an outside corner"
    );
}
