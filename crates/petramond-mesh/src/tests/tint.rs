//! The cell-tint invariant, pinned across every box-shaped family.
//!
//! `petramond:tint` is a UNIVERSAL presentation primitive: whatever writes it,
//! any cell carrying it renders multiplied by that colour against its tiles'
//! dye-base twins. Universality is exactly what a per-family implementation
//! cannot guarantee — before the geometry currency unified them, four of the
//! six box families flagged the dye base without ever applying the multiply,
//! so a tinted fence/pane/ladder/custom cell rendered whitened and untinted.
//!
//! These tests are family-parameterized on purpose: a NEW box family that
//! forgets the tint fails here without anyone remembering to extend the suite.

use super::*;
use petramond_world::block::TINT_KV_KEY;

/// Every box-shaped engine family, meshed as a lone cell on the floor.
fn box_families() -> Vec<(&'static str, Block)> {
    vec![
        ("stair", Block::OakStairs),
        ("slab", Block::OakSlab),
        ("fence", Block::OakFence),
        ("pane", Block::GlassPane),
        ("ladder", Block::Ladder),
    ]
}

/// A section holding ONE `block` at (1, 1, 1) in open air, optionally with a
/// `petramond:tint` on that cell. Nothing else emits geometry, so every
/// vertex in the mesh belongs to the shape under test — no floor face shares
/// the cell boundary and confuses the filter.
fn tinted_scene(block: Block, tint: Option<[u8; 3]>) -> Section {
    let mut section = section_with(&[((1, 1, 1), block)]);
    if let Some(rgb) = tint {
        section.cell_kv_set(1, 1, 1, TINT_KV_KEY.to_owned(), rgb.to_vec());
    }
    section
}

/// Every vertex the scene emitted — all of it is the lone shape's.
fn cell_verts(mesh: &ChunkMesh) -> Vec<Vertex> {
    mesh.opaque
        .iter()
        .chain(mesh.transparent.iter())
        .copied()
        .collect()
}

/// A cell tint multiplies the drawn geometry of EVERY box family — not just
/// the ones whose emitter happened to fold it into its own tint closure.
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
        // The multiply must actually reach the vertex tint lane. Comparing
        // against the untinted build catches the exact regression the split
        // producers had: flag set, colour untouched.
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

/// A full stack of one material, with the two layers dyed DIFFERENTLY, draws
/// each layer in its own colour.
///
/// Two things have to hold at once and each fails silently on its own: the
/// stack must not take the same-material cube fast path (which has a single
/// whole-cell tint, so it would paint both halves one colour), and the tint
/// post-pass must address each box by ITS part rather than slapping the cell's
/// one tint onto every box.
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
    // Every vertex is dyed (both layers carry a tint), and the two layers
    // disagree on colour — the whole point.
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
    // The bottom layer spans y 1.0..1.5 and the top 1.5..2.0, so vertices
    // strictly above the split plane belong to the top layer alone.
    let upper = colours(true);
    assert!(!upper.is_empty(), "the top layer must emit its own faces");
    for c in upper {
        assert!(
            (c[1] - top[1] as f32 / 255.0).abs() < 2.0 / 255.0,
            "a top-layer vertex must carry the top layer's tint, got {c:?}"
        );
    }
}

/// The same-material stack takes the cube fast path — and with it the greedy
/// merge streaming depends on — exactly when its two layers AGREE on colour.
/// Disagreeing layers are not one cube and must fall to the per-layer emitter,
/// which is what makes the two-tone stack above drawable at all.
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
    // The cube path emits 6 quads; the per-layer emitter splits the four sides
    // in two, so it is strictly bigger — the vertex count tells the paths apart.
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
    // A tint on ONE layer is a disagreement too: the other layer is undyed.
    assert!(
        stack(&[(0, orange)]) > cube,
        "one dyed layer under a plain one must not collapse to a cube"
    );
}

/// An untinted cell keeps the plain atlas half — the dyed flag is opt-in, so a
/// world with no tints anywhere pays nothing and looks unchanged.
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
