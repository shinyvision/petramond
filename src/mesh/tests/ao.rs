use super::*;
use crate::block_state::{SlabSplit, SlabState};
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

/// The unified box-set emitter's self-AO: a lone stair floating in air
/// darkens its own inner crease (the tread corners against the riser probe
/// into the upper box), while a lone full cube in the same empty air keeps
/// every corner at AO 3 — the probes reduce to the (empty) grid ring there.
#[test]
fn stair_crease_gets_self_ao_but_lone_cube_stays_open() {
    let m_cube = mesh(&section_with(&[((8, 8, 8), Block::Stone)]));
    assert!(
        m_cube.opaque.iter().all(|v| ao_idx(v) == 3),
        "a lone cube in air has no occluders"
    );

    let m_stair = mesh(&section_with(&[((8, 8, 8), Block::StoneStairs)]));
    // Only the crease darkens: some up-facing (shade 0) vertices on the
    // tread's mid line drop below 3; fully open corners elsewhere stay 3.
    let tread_creased = m_stair
        .opaque
        .iter()
        .any(|v| shade_idx(v) == 0 && ao_idx(v) < 3);
    assert!(
        tread_creased,
        "the tread corners against the riser must self-shadow"
    );
    assert!(
        m_stair.opaque.iter().any(|v| ao_idx(v) == 3),
        "open stair corners keep full AO"
    );
}

/// Sub-cell AO CASTING: a stair sitting on a floor darkens the neighbouring
/// floor cell's top-face corners toward it (the cast probes find the stair's
/// occupied half), while corners away from the stair stay fully lit.
#[test]
fn stair_casts_onto_the_terrain_beside_it() {
    let m = mesh(&section_with(&[
        ((7, 7, 8), Block::Stone),
        ((8, 7, 8), Block::Stone),
        ((8, 8, 8), Block::OakStairs),
    ]));
    let floor_top: Vec<_> = m
        .opaque
        .iter()
        .filter(|v| {
            shade_idx(v) == 0
                && (v.pos[1] - 8.0).abs() < 1.0e-3
                && v.pos[0] >= 7.0 - 1.0e-3
                && v.pos[0] <= 8.0 + 1.0e-3
        })
        .collect();
    // The x == 8 plane holds corners of BOTH floor faces: the exposed face's
    // (darkened by the cast) and the face buried under the stair (open —
    // its ring is the stair's own cell, never probed, exactly like grid AO).
    assert!(
        floor_top
            .iter()
            .any(|v| (v.pos[0] - 8.0).abs() < 1.0e-3 && ao_idx(v) < 3),
        "floor corners against the stair must darken"
    );
    assert!(
        floor_top
            .iter()
            .filter(|v| (v.pos[0] - 7.0).abs() < 1.0e-3)
            .all(|v| ao_idx(v) == 3),
        "floor corners away from the stair stay open"
    );
}

/// Fences are smooth-lit box sets like everything else: a lone post in air
/// has no occluders anywhere (probes find nothing — a centred post casts and
/// receives nothing), while a connected fence self-shadows the post/rail
/// junctions.
#[test]
fn fence_self_ao_at_rail_junctions_only() {
    let m_lone = mesh(&section_with(&[((8, 8, 8), Block::OakFence)]));
    assert!(
        m_lone.opaque.iter().all(|v| ao_idx(v) == 3),
        "a bare post in empty air keeps full AO everywhere"
    );

    let m_pair = mesh(&section_with(&[
        ((8, 8, 8), Block::OakFence),
        ((9, 8, 8), Block::OakFence),
    ]));
    assert!(
        m_pair.opaque.iter().any(|v| ao_idx(v) < 3),
        "the post/rail junctions must self-shadow"
    );
}


/// The cast probes are pocket VOLUMES, not points: a neighbour whose matter
/// is inset from the cell boundary (the cauldron's 1px-inset base) must
/// still occlude the side pocket of an edge-adjacent floor corner. Point
/// probes sat exactly on the corner line, missed the inset, and darkened
/// only the diagonal cells — hard corner splotches instead of a uniform
/// band along the shape's edge.
#[test]
fn cast_pockets_reach_an_inset_neighbour_base() {
    use crate::mesh::builder::corner_cast_probes;
    use crate::mesh::face::Face;
    // A floor's top face fronted by air at world (0,0,0); the shape sits in
    // the side cell (1,0,0) with its base inset 1/16 from every boundary.
    let base_lo = [1.0 / 16.0, 0.0, 1.0 / 16.0];
    let base_hi = [15.0 / 16.0, 3.0 / 16.0, 15.0 / 16.0];
    for sv in [-1, 1] {
        let pockets = corner_cast_probes(Face::PosY, (0, 0, 0), 1, sv);
        // The side-u pocket, moved into the side cell's local frame.
        let (lo, hi) = pockets[0];
        let (lo, hi) = ([lo[0] - 1.0, lo[1], lo[2]], [hi[0] - 1.0, hi[1], hi[2]]);
        let overlap = (0..3).all(|a| lo[a] < base_hi[a] && hi[a] > base_lo[a]);
        assert!(overlap, "sv={sv}: the edge pocket must reach the inset base");
    }
}

/// The INTERIOR quadrant: sub-cell matter standing ON a face darkens the
/// exposed part of that same face (its front cell holds the matter — grid
/// AO never probes there), and the quadrant-symmetric rule makes the shared
/// corner between the supporting face and the neighbouring floor face
/// compute the SAME level from both sides — no hard edge at the cell
/// boundary (the cauldron-gutter fix, exercised here with a vertical slab).
#[test]
fn matter_standing_on_a_face_darkens_it_seamlessly() {
    let mut section = section_with(&[
        ((7, 7, 8), Block::Stone),
        ((8, 7, 8), Block::Stone),
        ((8, 8, 8), Block::StoneSlab),
    ]);
    section.set_slab_state(
        8,
        8,
        8,
        SlabState {
            split: SlabSplit::X,
            layers: [Block::StoneSlab, Block::Air],
        },
    );
    let m = mesh(&section);

    // The vertical slab occupies the west half of its cell. At the shared
    // boundary x = 8 every top-face corner (supporting face AND neighbour
    // face) must agree and darken; the far corners of both faces stay open.
    let floor_top: Vec<_> = m
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 0 && (v.pos[1] - 8.0).abs() < 1.0e-3)
        .collect();
    let at_x = |x: f32| floor_top.iter().filter(move |v| (v.pos[0] - x).abs() < 1.0e-3);
    let shared: Vec<u32> = at_x(8.0).map(|v| ao_idx(v)).collect();
    assert!(!shared.is_empty());
    assert!(
        shared.iter().all(|&a| a == shared[0]),
        "both faces must shade the shared boundary identically: {shared:?}"
    );
    assert!(shared[0] < 3, "the boundary corners must darken toward the slab");
    assert!(
        at_x(7.0).chain(at_x(9.0)).all(|v| ao_idx(v) == 3),
        "far corners of both faces stay open"
    );
}
