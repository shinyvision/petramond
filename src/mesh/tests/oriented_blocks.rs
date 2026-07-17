use super::*;
use crate::block_state::LogAxis;

#[test]
fn horizontal_log_bark_faces_use_axis_aligned_cell_local_uvs() {
    use crate::mesh::vertex::{UV_MODE_CELL_LOCAL, UV_MODE_NONE};

    let mut section = section_with(&[((8, 8, 8), Block::OakLog)]);
    section.set_log_axis(8, 8, 8, LogAxis::X);
    let m = mesh(&section);

    let top_bark = m
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 0 && (v.pos[1] - 9.0).abs() < 1.0e-3)
        .collect::<Vec<_>>();
    assert_eq!(top_bark.len(), 4, "top bark face should emit one quad");
    assert!(
        top_bark.iter().all(|v| uv_mode(v) == UV_MODE_CELL_LOCAL),
        "horizontal log bark faces must carry explicit UVs"
    );
    let mut uvs = top_bark.iter().map(|v| cell_uv16(v)).collect::<Vec<_>>();
    uvs.sort_unstable();
    assert_eq!(
        uvs,
        vec![(0, 0), (0, 16), (16, 0), (16, 16)],
        "the rotated bark face must still span the full tile"
    );

    let end_caps = m
        .opaque
        .iter()
        .filter(|v| {
            shade_idx(v) == 2
                && ((v.pos[0] - 8.0).abs() < 1.0e-3 || (v.pos[0] - 9.0).abs() < 1.0e-3)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        end_caps.len(),
        8,
        "x-axis log should have two end-cap quads"
    );
    assert!(
        end_caps.iter().all(|v| uv_mode(v) == UV_MODE_NONE),
        "log end caps keep the normal cube UV mapping"
    );
}

/// A placed furnace shows its front on exactly the face it was placed facing and
/// `furnace_side` on the other three horizontal faces (top + bottom use the top
/// tile). Pins the directional fix for "front rendered on all four sides".
#[test]
fn furnace_shows_front_on_facing_face_and_side_on_the_others() {
    use crate::atlas::Tile;
    use crate::furnace::Furnace;

    let mut section = section_with(&[((8, 8, 8), Block::Furnace)]);
    section.insert_furnace(8, 8, 8, Furnace::default());
    section.insert_entity_facing(8, 8, 8, Facing::East);

    let count = |mesh: &ChunkMesh, tile: Tile| {
        mesh.opaque
            .iter()
            .filter(|v| tile_idx(v) == tile.index() as u32)
            .count()
    };

    // Unlit: 6 faces × 4 verts — 1 front, 3 sides, 2 top/bottom.
    let m = mesh(&section);
    assert_eq!(
        count(&m, Tile::named("furnace_front")),
        4,
        "front on exactly the facing face"
    );
    assert_eq!(
        count(&m, Tile::named("furnace_side")),
        12,
        "side on the other three faces"
    );
    assert_eq!(count(&m, Tile::named("furnace_top")), 8, "top + bottom");
    assert_eq!(
        count(&m, Tile::named("furnace_front_on")),
        0,
        "no lit front while unlit"
    );

    // Lit: the facing face swaps to the glowing front; the sides do not glow.
    section.insert_furnace(
        8,
        8,
        8,
        Furnace {
            burn_remaining: 100,
            ..Default::default()
        },
    );
    let lit = mesh(&section);
    assert_eq!(
        count(&lit, Tile::named("furnace_front_on")),
        4,
        "lit front on the facing face only"
    );
    assert_eq!(count(&lit, Tile::named("furnace_front")), 0);
    assert_eq!(
        count(&lit, Tile::named("furnace_side")),
        12,
        "sides never glow"
    );
}
