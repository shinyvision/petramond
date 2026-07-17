use super::*;
use crate::mesh::face::{should_flip, vertex_ao};

/// Leaves occlude AO onto/within themselves: a solid leaf cluster floating in
/// air must produce darkened (ao < 3) leaf faces -- interior faces are buried
/// by surrounding leaves. (Before, leaves never occluded, so AO stayed 3.)
#[test]
fn leaves_self_occlude() {
    assert!(Block::OakLeaves.occludes_ao());
    assert!(!Block::Water.occludes_ao());
    assert!(!Block::Air.occludes_ao());

    let mut section = Section::new(0, 0, 0);
    for y in 5..=7 {
        for z in 7..=9 {
            for x in 7..=9 {
                section.set_block(x, y, z, Block::OakLeaves);
            }
        }
    }
    let m = mesh(&section);
    assert!(
        !m.opaque.is_empty(),
        "leaf cluster should mesh (cutout opaque pass)"
    );
    let min_ao = m.opaque.iter().map(ao_idx).min().unwrap();
    assert!(
        min_ao < 3,
        "leaves in a cluster must self-occlude (some ao < 3)"
    );
}

/// The AO occlusion table: brightest with no occluders, one step per single
/// occluder, and the buried-corner special case (both edges solid -> 0).
#[test]
fn vertex_ao_levels() {
    assert_eq!(vertex_ao(false, false, false), 3); // open
    assert_eq!(vertex_ao(true, false, false), 2); // one edge
    assert_eq!(vertex_ao(false, false, true), 2); // diagonal only
    assert_eq!(vertex_ao(true, false, true), 1); // edge + diagonal
    assert_eq!(vertex_ao(true, true, false), 0); // both edges -> buried
    assert_eq!(vertex_ao(true, true, true), 0); // both edges, diagonal irrelevant
}

/// Flip exactly when the 0-2 diagonal is the brighter pair; ties keep default.
#[test]
fn flip_runs_along_darker_diagonal() {
    assert!(should_flip([3, 0, 3, 0])); // 0-2 bright (6) vs 1-3 dark (0) -> flip
    assert!(!should_flip([0, 3, 0, 3])); // 1-3 brighter -> keep default
    assert!(!should_flip([3, 3, 3, 3])); // symmetric -> no flip
    assert!(!should_flip([2, 1, 1, 2])); // equal sums (3 == 3) -> no flip
}

/// AO produces the exact occlusion contract at a known concave corner, on a
/// hand-built fixture (no worldgen coupling). A 1-tall step block sits beside a
/// 2-tall pillar one cell over in +X; the pillar's upper cube edge-occludes the
/// step block's TOP face along its shared +X edge. The two top corners on that
/// edge therefore read ao == 2 (one solid edge neighbour:
/// `vertex_ao(true, false, false)`), while the two corners on the open -X edge
/// stay at the un-occluded ao == 3. The precise table is pinned separately by
/// `vertex_ao_levels`; this proves the builder feeds it the right neighbourhood.
#[test]
fn ao_exact_at_concave_step_corner() {
    let m = mesh(&section_with(&[
        // The step block.
        ((8, 8, 8), Block::Stone),
        // The 2-tall pillar one cell over in +X; its upper cube (9,9,8) is the
        // single edge-occluder of the step block's top (+Y) face.
        ((9, 8, 8), Block::Stone),
        ((9, 9, 8), Block::Stone),
    ]));

    // The step block's top face is the only +Y (PosY -> shade idx 0) quad whose
    // four corners lie at y == 9 over the step cell x in [8,9], z in [8,9].
    let ao_at = |wx: f32, wz: f32| ao_idx(vert_at(&m.opaque, 0, [wx, 9.0, wz]));

    // The two corners on the shared +X edge (x == 9), adjacent to the pillar:
    // one solid edge neighbour each -> ao == 2.
    assert_eq!(ao_at(9.0, 8.0), 2, "concave +X corner is edge-occluded");
    assert_eq!(ao_at(9.0, 9.0), 2, "concave +X corner is edge-occluded");
    // The two corners on the open -X edge (x == 8): no occluder -> ao == 3.
    assert_eq!(ao_at(8.0, 8.0), 3, "open -X corner is fully lit");
    assert_eq!(ao_at(8.0, 9.0), 3, "open -X corner is fully lit");
}
