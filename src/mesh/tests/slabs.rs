use super::*;
use crate::block_state::SlabSplit;

/// A full slab stack of ONE material must mesh exactly like the full cube block:
/// same quads, same culling (the floor face under it disappears), same AO onto
/// the surrounding floor, same greedy merging — the tile id is the only thing
/// allowed to differ, so it is masked out of the comparison.
#[test]
fn uniform_full_slab_stack_meshes_like_the_full_cube_block() {
    let build = |cube: bool| {
        let mut section = floor_section(Block::Dirt);
        if cube {
            section.set_block(5, 1, 5, Block::Stone);
        } else {
            section.set_block(5, 1, 5, Block::StoneSlab);
            section.set_slab_state(
                5,
                1,
                5,
                SlabState {
                    split: SlabSplit::Y,
                    layers: [Block::StoneSlab, Block::StoneSlab],
                },
            );
        }
        mesh(&section)
    };
    let scrub_tile = |mesh: &ChunkMesh| -> Vec<Vertex> {
        mesh.opaque
            .iter()
            .map(|v| Vertex {
                packed: v.packed & !0xFF,
                ..*v
            })
            .collect()
    };

    let slab = build(false);
    let cube = build(true);
    assert!(!slab.opaque.is_empty());
    assert_eq!(
        bytemuck::cast_slice::<Vertex, u8>(&scrub_tile(&slab)),
        bytemuck::cast_slice::<Vertex, u8>(&scrub_tile(&cube)),
        "a same-material full stack must emit the full cube block's exact mesh"
    );
    assert_eq!(slab.opaque_idx, cube.opaque_idx);
}

/// A mixed-material full stack still covers the whole cell like a full block but
/// keeps each layer's texture: full caps textured by the fronting layer, split
/// side faces carrying both materials' side tiles.
#[test]
fn mixed_full_slab_stack_covers_the_cell_with_both_layer_tiles() {
    let mut section = section_with(&[((8, 8, 8), Block::StoneSlab)]);
    section.set_slab_state(
        8,
        8,
        8,
        SlabState {
            split: SlabSplit::Y,
            layers: [Block::StoneSlab, Block::DirtSlab],
        },
    );
    let m = mesh(&section);

    // Full-cell cover: 2 full caps + 4 sides x 2 half quads = 10 quads.
    assert_eq!(m.opaque.len(), 40, "mixed stack should emit 10 quads");
    let tiles_at = |pred: &dyn Fn(&Vertex) -> bool| -> std::collections::HashSet<u32> {
        m.opaque.iter().filter(|v| pred(v)).map(tile_idx).collect()
    };
    let top = tiles_at(&|v| shade_idx(v) == 0);
    let bottom = tiles_at(&|v| shade_idx(v) == 3);
    let sides = tiles_at(&|v| shade_idx(v) != 0 && shade_idx(v) != 3);
    assert_eq!(
        top,
        [Block::DirtSlab.tiles()[0].index() as u32].into(),
        "top cap must use the upper layer's top tile"
    );
    assert_eq!(
        bottom,
        [Block::StoneSlab.tiles()[1].index() as u32].into(),
        "bottom cap must use the lower layer's bottom tile"
    );
    assert_eq!(
        sides,
        [
            Block::StoneSlab.tiles()[2].index() as u32,
            Block::DirtSlab.tiles()[2].index() as u32,
        ]
        .into(),
        "side faces must keep both layers' side tiles"
    );
}

/// Full stacks cull like opaque cubes, in BOTH directions: the boundary between
/// a full (mixed) stack and an adjacent opaque cube must emit no quad at all.
#[test]
fn faces_between_a_full_slab_stack_and_an_opaque_cube_are_culled() {
    let mut section = section_with(&[((8, 8, 8), Block::StoneSlab), ((9, 8, 8), Block::Stone)]);
    section.set_slab_state(
        8,
        8,
        8,
        SlabState {
            split: SlabSplit::Y,
            layers: [Block::StoneSlab, Block::DirtSlab],
        },
    );
    let m = mesh(&section);

    for quad in m.opaque.chunks(4) {
        assert!(
            !quad.iter().all(|v| (v.pos[0] - 9.0).abs() < 1e-3),
            "no quad may sit on the stack/cube boundary plane (stack side {:?})",
            quad.iter().map(|v| v.pos).collect::<Vec<_>>()
        );
    }
}

/// The screenshot regression: a wall face rising from a top-slab floor must not
/// blend the slabs' under-floor darkness into its bottom corners. A slab cell's
/// light value describes its OPEN half; the wall corners rest on the slabs'
/// SOLID top half, which seals that darkness away, so the wall must shade
/// exactly like one standing on full blocks.
#[test]
fn wall_face_above_a_top_slab_floor_ignores_under_slab_darkness() {
    let mut section = Section::new(0, 0, 0);
    // A stone wall along z=7 rising one block above a top-slab floor that
    // fills z=8..=10, with a solid base under the wall.
    for x in 6..=10 {
        for y in 7..=9 {
            section.set_block(x, y, 7, Block::Stone);
        }
        for z in 8..=10 {
            section.set_block(x, 8, z, Block::StoneSlab);
            section.set_slab_state(
                x,
                8,
                z,
                SlabState::single(SlabSplit::Y, 1, Block::StoneSlab),
            );
        }
    }

    // Baked-light shape: the slab cells and the space below them carry the
    // dark under-floor light; everything above and beside is fully sky-lit.
    let m = mesh_with_sky(&section, |wx, wy, wz| {
        if wy <= 8 && (6..=10).contains(&wx) && (8..=10).contains(&wz) {
            0
        } else {
            SKY_FULL
        }
    });

    // The wall's face over the floor: quads on the z=8 plane wholly above the
    // slab tops (the buried faces below the floor surface keep their darkness).
    let mut checked = 0;
    for quad in m.opaque.chunks(4) {
        if !quad
            .iter()
            .all(|v| (v.pos[2] - 8.0).abs() < 1e-3 && v.pos[1] >= 9.0 - 1e-3)
        {
            continue;
        }
        for v in quad {
            assert_eq!(
                light6(v),
                63,
                "wall corner at {:?} must not sample the darkness sealed under the slab floor",
                v.pos
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 4,
        "expected the wall face above the floor to be emitted"
    );
}
