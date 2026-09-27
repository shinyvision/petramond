use super::*;
use petramond_world::block::TINT_KV_KEY;

fn box_families() -> Vec<(&'static str, Block)> {
    vec![
        ("stair", Block::OakStairs),
        ("slab", Block::OakSlab),
        ("fence", Block::OakFence),
        ("pane", Block::GlassPane),
        ("ladder", Block::Ladder),
    ]
}

fn tinted_scene(block: Block, tint: Option<[u8; 3]>) -> Section {
    let mut section = section_with(&[((1, 1, 1), block)]);
    if let Some(rgb) = tint {
        section.cell_kv_set(1, 1, 1, TINT_KV_KEY.to_owned(), rgb.to_vec());
    }
    section
}

fn cell_verts(mesh: &ChunkMesh) -> Vec<Vertex> {
    mesh.opaque
        .iter()
        .chain(mesh.transparent.iter())
        .copied()
        .collect()
}

#[test]
fn a_cell_tint_multiplies_every_box_family() {
    let tint = [128u8, 64, 32];
    for (name, block) in box_families() {
        let plain = cell_verts(&mesh(&tinted_scene(block, None)));
        let dyed = cell_verts(&mesh(&tinted_scene(block, Some(tint))));
        assert!(
            !plain.is_empty(),
            "{name}: the lone shape must emit geometry"
        );
        assert_eq!(
            plain.len(),
            dyed.len(),
            "{name}: a tint changes colour, never geometry"
        );
        assert!(
            dyed.iter().all(|v| v.packed2 & crate::DYED_FLAG2 != 0),
            "{name}: every tinted vertex samples the dye-base twin"
        );
        for (p, d) in plain.iter().zip(dyed.iter()) {
            let before = crate::unpack_tint(p.tint);
            let after = crate::unpack_tint(d.tint);
            for c in 0..3 {
                let expect = before[c] * tint[c] as f32 / 255.0;
                assert!(
                    (after[c] - expect).abs() < 2.0 / 255.0,
                    "{name}: channel {c} tint {after:?} is not {before:?} × {tint:?}"
                );
            }
        }
    }
}

#[test]
fn a_stack_dyed_per_layer_draws_each_layer_in_its_own_colour() {
    let bottom = [255u8, 255, 255];
    let top = [255u8, 96, 0];
    let mut section = section_with(&[((1, 1, 1), Block::WoolSlab)]);
    section.set_slab_state(
        1,
        1,
        1,
        SlabState {
            split: petramond_world::block_state::SlabSplit::Y,
            layers: [Block::WoolSlab, Block::WoolSlab],
        },
    );
    section.cell_kv_set(1, 1, 1, TINT_KV_KEY.to_owned(), bottom.to_vec());
    section.cell_kv_set(
        1,
        1,
        1,
        petramond_world::block::part_kv_key(TINT_KV_KEY, 1),
        top.to_vec(),
    );

    let verts = cell_verts(&mesh(&section));
    assert!(!verts.is_empty(), "the stack must emit geometry");
    assert!(
        verts.iter().all(|v| v.packed2 & crate::DYED_FLAG2 != 0),
        "both dyed layers sample the dye-base twin"
    );
    let colours = |above: bool| -> Vec<[f32; 3]> {
        verts
            .iter()
            .filter(|v| (v.pos[1] > 1.5 + 1e-3) == above)
            .map(|v| crate::unpack_tint(v.tint))
            .collect()
    };
    let upper = colours(true);
    assert!(!upper.is_empty(), "the top layer must emit its own faces");
    for c in upper {
        assert!(
            (c[1] - top[1] as f32 / 255.0).abs() < 2.0 / 255.0,
            "a top-layer vertex must carry the top layer's tint, got {c:?}"
        );
    }
}

#[test]
fn the_cube_fast_path_follows_whether_the_layers_agree_on_colour() {
    let stack = |tints: &[(petramond_world::block::CellPart, [u8; 3])]| {
        let mut section = section_with(&[((1, 1, 1), Block::WoolSlab)]);
        section.set_slab_state(
            1,
            1,
            1,
            SlabState {
                split: petramond_world::block_state::SlabSplit::Y,
                layers: [Block::WoolSlab, Block::WoolSlab],
            },
        );
        for &(part, rgb) in tints {
            section.cell_kv_set(
                1,
                1,
                1,
                petramond_world::block::part_kv_key(TINT_KV_KEY, part),
                rgb.to_vec(),
            );
        }
        cell_verts(&mesh(&section)).len()
    };
    let orange = [255u8, 96, 0];
    let white = [255u8, 255, 255];
    let cube = stack(&[]);
    assert_eq!(
        stack(&[(0, orange), (1, orange)]),
        cube,
        "layers that agree on colour stay on the cube path"
    );
    assert!(
        stack(&[(0, white), (1, orange)]) > cube,
        "layers that disagree must fall to the per-layer emitter"
    );
    assert!(
        stack(&[(0, orange)]) > cube,
        "one dyed layer under a plain one must not collapse to a cube"
    );
}

#[test]
fn an_untinted_cell_never_sets_the_dyed_flag() {
    for (name, block) in box_families() {
        let verts = cell_verts(&mesh(&tinted_scene(block, None)));
        assert!(
            verts.iter().all(|v| v.packed2 & crate::DYED_FLAG2 == 0),
            "{name}: an untinted cell must sample the plain tile"
        );
    }
}
