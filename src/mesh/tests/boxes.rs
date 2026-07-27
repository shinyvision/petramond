use super::*;

/// Every +Y quad on the plane `y`, as (min, max) in x/z.
fn top_quads_at(m: &ChunkMesh, y: f32) -> Vec<([f32; 2], [f32; 2])> {
    m.opaque
        .chunks_exact(4)
        .filter(|q| shade_idx(&q[0]) == 0 && q.iter().all(|v| (v.pos[1] - y).abs() < 1e-3))
        .map(|q| {
            let fold = |a: usize| {
                q.iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), v| {
                        (l.min(v.pos[a]), h.max(v.pos[a]))
                    })
            };
            let (x0, x1) = fold(0);
            let (z0, z1) = fold(2);
            ([x0, z0], [x1, z1])
        })
        .collect()
}

fn covers(q: &([f32; 2], [f32; 2]), x: f32, z: f32) -> bool {
    q.0[0] < x && x < q.1[0] && q.0[1] < z && z < q.1[1]
}

/// A CAP PLATE flush with the body it caps must still draw its face. The
/// coincidence tie-break only settles which of two boxes draws a shared plane;
/// a box that never emits that face has no claim on it. Getting that backwards
/// made the cactus top vanish entirely (2026-07-25 playtest).
///
/// The same fixture pins the inset shape's other two playtest bugs: it must
/// NOT seal the boundary under it (or you see through the terrain past the
/// recess), and its own side faces must sit inset rather than on the cell wall.
#[test]
fn an_inset_shape_caps_its_top_without_sealing_the_ground_under_it() {
    let mut section = floor_section(Block::Sand);
    section.set_block(8, 1, 8, Block::Cactus);
    let m = mesh(&section);

    let cap = top_quads_at(&m, 2.0);
    assert!(
        cap.iter().any(|q| covers(q, 8.5, 8.5)),
        "the cap plate's top face must draw over the body it caps, got {cap:?}"
    );

    // The ground it stands on keeps its top face: the trunk is inset, so it
    // seals nothing, and a culled carrier top would show through the recess.
    let ground = top_quads_at(&m, 1.0);
    assert!(
        ground.iter().any(|q| covers(q, 8.5, 8.5)),
        "an inset shape must not cull the top of the block it stands on"
    );

    // The side faces are the INSET planes, never the cell walls (the exact
    // inset plane is pinned by the next test).
    let on_the_cell_wall = m.opaque.chunks_exact(4).any(|q| {
        q.iter()
            .all(|v| (v.pos[0] - 9.0).abs() < 1e-3 && v.pos[1] >= 1.0 && v.pos[1] <= 2.0)
    });
    assert!(
        !on_the_cell_wall,
        "an inset shape must not draw a face on the cell wall"
    );
}

/// An inset shape's side faces span the WHOLE cell, not just the body behind
/// them. A 14-wide face leaves the four corner columns open and you see
/// straight through the block (2026-07-25 playtest) — and, because the face
/// spans the cell, its cell-local UV covers the tile edge to edge, which is
/// what keeps art drawn at the tile's edges (the cactus spines) on screen.
#[test]
fn an_inset_face_spans_the_whole_cell_so_no_corner_is_left_open() {
    let mut section = floor_section(Block::Sand);
    section.set_block(8, 1, 8, Block::Cactus);
    let m = mesh(&section);

    // The +X face sits a texel in from the wall, and covers the cell's full
    // z extent — corner to corner.
    let face: Vec<&[Vertex]> = m
        .opaque
        .chunks_exact(4)
        .filter(|q| {
            q.iter()
                .all(|v| (v.pos[0] - (8.0 + 15.0 / 16.0)).abs() < 1e-3)
        })
        .collect();
    assert!(!face.is_empty(), "expected the inset +X face");
    let (z0, z1) = face
        .iter()
        .flat_map(|q| q.iter())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), v| {
            (l.min(v.pos[2]), h.max(v.pos[2]))
        });
    assert!(
        (z0 - 8.0).abs() < 1e-3 && (z1 - 9.0).abs() < 1e-3,
        "an inset face must still span the cell corner to corner, got {z0}..{z1}"
    );

    // Spanning the cell is also what makes it sample the whole tile.
    let us: Vec<u32> = face
        .iter()
        .flat_map(|q| q.iter().map(|v| cell_uv16(v).0))
        .collect();
    assert!(
        us.contains(&0) && us.contains(&16),
        "a full-width face samples the tile edge to edge, got {us:?}"
    );
}

/// A DOUBLE-SIDED box face appends its four corners a second time in reverse
/// order, so the same plane survives back-face culling from either side. The
/// cactus's side tile is transparent along its edge columns except where the
/// spines poke out, so through the near face's notches you must see the far
/// face's spines rather than the sky — single-sided, half the spines vanish as
/// you strafe past.
#[test]
fn a_double_sided_face_emits_both_windings_over_one_set_of_vertices() {
    let mut section = floor_section(Block::Sand);
    section.set_block(8, 1, 8, Block::Cactus);
    let m = mesh(&section);

    // The inset +X face: the same four corner positions appear as TWO quads in
    // the vertex buffer, and the implied triangulation gives them opposite
    // windings (the second quad's corner order is the first's reversed).
    let quads: Vec<usize> = m
        .opaque
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, q)| {
            q.iter()
                .all(|v| (v.pos[0] - (8.0 + 15.0 / 16.0)).abs() < 1e-3)
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        quads.len(),
        2,
        "expected the inset +X face twice, got {quads:?}"
    );
    let front: Vec<[f32; 3]> = m.opaque[quads[0] * 4..quads[0] * 4 + 4]
        .iter()
        .map(|v| v.pos)
        .collect();
    let back: Vec<[f32; 3]> = m.opaque[quads[1] * 4..quads[1] * 4 + 4]
        .iter()
        .map(|v| v.pos)
        .collect();
    assert_eq!(
        back,
        [front[0], front[3], front[2], front[1]],
        "the back copy must be the front corners in reverse order"
    );
}

/// A box that carries a face without being MATTER must not shadow or seal:
/// the cactus's side planes span the cell so their faces are full width, but
/// the body is the inset trunk. Treating those planes as matter shadows the
/// ground like a full cube and seals the cell's own light out.
#[test]
fn a_face_only_box_is_not_matter() {
    use crate::block::light_aperture_face;
    let k = Block::Cactus.shape_kind().def();
    let occupied = |lo: [f32; 3], hi: [f32; 3]| {
        k.sim.occupies_pocket(
            &k.params,
            &crate::block::NoNeighborhood,
            crate::mathh::IVec3::ZERO,
            Block::Cactus,
            lo,
            hi,
        )
    };
    assert!(
        !occupied([0.0, 0.0, 0.4], [1.0 / 16.0, 0.2, 0.6]),
        "the cell wall holds a face plane, not matter"
    );
    assert!(
        occupied([0.4, 0.0, 0.4], [0.6, 0.2, 0.6]),
        "the inset trunk is the matter"
    );
    let sides = light_aperture_face(Block::Cactus.default_light_apertures(), (1, 0, 0));
    assert_eq!(sides, 0b1111, "its recessed sides stay open to the light");

    // ...and a neighbour's flush face is not culled against a face carrier:
    // the plane reaches the cell wall, but nothing is drawn there.
    let mut section = floor_section(Block::Sand);
    section.set_block(8, 1, 8, Block::Cactus);
    section.set_block(9, 1, 8, Block::StoneSlab);
    section.set_slab_state(
        9,
        1,
        8,
        SlabState::single(crate::block_state::SlabSplit::Y, 0, Block::StoneSlab),
    );
    let m = mesh(&section);
    let slab_face_toward_cactus = m.opaque.chunks_exact(4).any(|q| {
        q.iter()
            .all(|v| (v.pos[0] - 9.0).abs() < 1e-3 && v.pos[1] >= 1.0 && v.pos[1] <= 1.5)
    });
    assert!(
        slab_face_toward_cactus,
        "a neighbour must keep the face a bare face carrier stands against"
    );
}
