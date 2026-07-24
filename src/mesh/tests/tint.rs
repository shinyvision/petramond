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
use crate::block::TINT_KV_KEY;

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
            dyed.iter()
                .all(|v| v.packed2 & crate::mesh::DYED_FLAG2 != 0),
            "{name}: every tinted vertex samples the dye-base twin"
        );
        // The multiply must actually reach the vertex tint lane. Comparing
        // against the untinted build catches the exact regression the split
        // producers had: flag set, colour untouched.
        for (p, d) in plain.iter().zip(dyed.iter()) {
            let before = crate::mesh::unpack_tint(p.tint);
            let after = crate::mesh::unpack_tint(d.tint);
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

/// An untinted cell keeps the plain atlas half — the dyed flag is opt-in, so a
/// world with no tints anywhere pays nothing and looks unchanged.
#[test]
fn an_untinted_cell_never_sets_the_dyed_flag() {
    for (name, block) in box_families() {
        let verts = cell_verts(&mesh(&tinted_scene(block, None)));
        assert!(
            verts
                .iter()
                .all(|v| v.packed2 & crate::mesh::DYED_FLAG2 == 0),
            "{name}: an untinted cell must sample the plain tile"
        );
    }
}
