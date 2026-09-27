use super::*;
use crate::face::{should_flip, vertex_ao};
use petramond_world::block_state::{SlabSplit, SlabState};

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

#[test]
fn vertex_ao_levels() {
    assert_eq!(vertex_ao(false, false, false), 3);
    assert_eq!(vertex_ao(true, false, false), 2);
    assert_eq!(vertex_ao(false, false, true), 2);
    assert_eq!(vertex_ao(true, false, true), 1);
    assert_eq!(vertex_ao(true, true, false), 0);
    assert_eq!(vertex_ao(true, true, true), 0);
}

#[test]
fn flip_runs_along_darker_diagonal() {
    assert!(should_flip([3, 0, 3, 0]));
    assert!(!should_flip([0, 3, 0, 3]));
    assert!(!should_flip([3, 3, 3, 3]));
    assert!(!should_flip([2, 1, 1, 2]));
}

#[test]
fn ao_exact_at_concave_step_corner() {
    let m = mesh(&section_with(&[
        ((8, 8, 8), Block::Stone),
        ((9, 8, 8), Block::Stone),
        ((9, 9, 8), Block::Stone),
    ]));

    let ao_at = |wx: f32, wz: f32| ao_idx(vert_at(&m.opaque, 0, [wx, 9.0, wz]));

    assert_eq!(ao_at(9.0, 8.0), 2, "concave +X corner is edge-occluded");
    assert_eq!(ao_at(9.0, 9.0), 2, "concave +X corner is edge-occluded");
    assert_eq!(ao_at(8.0, 8.0), 3, "open -X corner is fully lit");
    assert_eq!(ao_at(8.0, 9.0), 3, "open -X corner is fully lit");
}

#[test]
fn stair_crease_gets_self_ao_but_lone_cube_stays_open() {
    let m_cube = mesh(&section_with(&[((8, 8, 8), Block::Stone)]));
    assert!(
        m_cube.opaque.iter().all(|v| ao_idx(v) == 3),
        "a lone cube in air has no occluders"
    );

    let m_stair = mesh(&section_with(&[((8, 8, 8), Block::StoneStairs)]));
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
    use crate::builder::corner_cast_probes;
    use crate::face::Face;
    let base_lo = [1.0 / 16.0, 0.0, 1.0 / 16.0];
    let base_hi = [15.0 / 16.0, 3.0 / 16.0, 15.0 / 16.0];
    for sv in [-1, 1] {
        let pockets = corner_cast_probes(Face::PosY, 1, sv, 0.0);
        let (lo, hi) = pockets[0];
        let (lo, hi) = ([lo[0] - 1.0, lo[1], lo[2]], [hi[0] - 1.0, hi[1], hi[2]]);
        let overlap = (0..3).all(|a| lo[a] < base_hi[a] && hi[a] > base_lo[a]);
        assert!(
            overlap,
            "sv={sv}: the edge pocket must reach the inset base"
        );
    }
}

/// A box family's INTERIOR plane takes AO from matter in FRONT of the plane
/// only: a bottom slab's top face (y = 0.5) ignores its own bottom-half
/// matter and the flush tops of neighbouring bottom slabs (a half-height
/// floor is one continuous surface — no per-slab darkening), while a
/// neighbouring TOP slab, whose matter rises above the plane, darkens the
/// corners toward it exactly like a wall. Probe pockets must follow the
/// actual plane rather than the cell floor.
#[test]
fn slab_top_plane_ignores_matter_below_it() {
    let top_face_ao = |m: &ChunkMesh, x: f32| -> Vec<u32> {
        m.opaque
            .iter()
            .filter(|v| {
                shade_idx(v) == 0
                    && (v.pos[1] - 8.5).abs() < 1e-3
                    && v.pos[0] >= x - 1e-3
                    && v.pos[0] <= x + 1.0 + 1e-3
            })
            .map(ao_idx)
            .collect()
    };

    let m_lone = mesh(&section_with(&[((8, 8, 8), Block::StoneSlab)]));
    let lone = top_face_ao(&m_lone, 8.0);
    assert!(!lone.is_empty());
    assert!(
        lone.iter().all(|&a| a == 3),
        "a lone slab's own bottom half must not shadow its top: {lone:?}"
    );

    let m_floor = mesh(&section_with(&[
        ((7, 8, 8), Block::StoneSlab),
        ((8, 8, 8), Block::StoneSlab),
        ((9, 8, 8), Block::StoneSlab),
        ((8, 8, 7), Block::StoneSlab),
        ((8, 8, 9), Block::StoneSlab),
    ]));
    let center = top_face_ao(&m_floor, 8.0);
    assert!(!center.is_empty());
    assert!(
        center.iter().all(|&a| a == 3),
        "flush bottom-slab neighbours form one continuous floor: {center:?}"
    );

    let mut section = section_with(&[((8, 8, 8), Block::StoneSlab), ((9, 8, 8), Block::StoneSlab)]);
    section.set_slab_state(
        9,
        8,
        8,
        SlabState::single(SlabSplit::Y, 1, Block::StoneSlab),
    );
    let m_wall = mesh(&section);
    let beside_wall = top_face_ao(&m_wall, 8.0);
    assert!(
        beside_wall.iter().any(|&a| a < 3),
        "a top slab rises above the plane and must darken toward it: {beside_wall:?}"
    );
}

#[test]
fn stair_corner_ao_is_placement_independent() {
    let build = |corner_facing: Facing| {
        let mut s = section_with(&[
            ((9, 8, 8), Block::OakStairs),
            ((10, 8, 8), Block::OakStairs),
            ((9, 8, 9), Block::OakStairs),
        ]);
        s.set_stair_facing(9, 8, 8, corner_facing);
        s.set_stair_facing(10, 8, 8, Facing::North);
        s.set_stair_facing(9, 8, 9, Facing::West);
        s
    };
    let a = build(Facing::West);
    let b = build(Facing::North);

    let corner_a = refined(&a).cell_state(9, 8, 8).byte(1);
    let corner_b = refined(&b).cell_state(9, 8, 8).byte(1);
    assert_eq!(corner_a, corner_b, "both placements refine to one corner");
    assert_eq!(corner_a.count_ones(), 1, "the corner is an outer corner");

    let ao_set = |m: &ChunkMesh| {
        let mut set: Vec<_> = m
            .opaque
            .iter()
            .map(|v| {
                let q = v.pos.map(|c| (c * 4.0).round() as i32);
                (q, shade_idx(v), ao_idx(v))
            })
            .collect();
        set.sort_unstable();
        set
    };
    assert_eq!(
        ao_set(&mesh(&a)),
        ao_set(&mesh(&b)),
        "identical refined shapes must shade identically regardless of placement"
    );
}

/// Interior quadrant. Sub-cell matter on a face should darken the rest of that face, and the
/// corner it shares with the neighbouring floor face should match from both sides. That was the
/// cauldron-gutter fix; a vertical slab exercises it here.
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

    let floor_top: Vec<_> = m
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 0 && (v.pos[1] - 8.0).abs() < 1.0e-3)
        .collect();
    let at_x = |x: f32| {
        floor_top
            .iter()
            .filter(move |v| (v.pos[0] - x).abs() < 1.0e-3)
    };
    let shared: Vec<u32> = at_x(8.0).map(|v| ao_idx(v)).collect();
    assert!(!shared.is_empty());
    assert!(
        shared.iter().all(|&a| a == shared[0]),
        "both faces must shade the shared boundary identically: {shared:?}"
    );
    assert!(
        shared[0] < 3,
        "the boundary corners must darken toward the slab"
    );
    assert!(
        at_x(7.0).chain(at_x(9.0)).all(|v| ao_idx(v) == 3),
        "far corners of both faces stay open"
    );
}
