use super::*;

#[test]
fn a_full_cube_under_a_snow_layer_culls_its_top_face() {
    let mut section = floor_section(Block::Stone);
    section.set_block(8, 1, 8, Block::SnowLayer);
    let m = mesh(&section);

    let quads: Vec<&[Vertex]> = m.opaque.chunks_exact(4).collect();
    let covers = |q: &[Vertex], x: f32, z: f32| {
        let (mut xmin, mut xmax, mut zmin, mut zmax) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        for v in q {
            xmin = xmin.min(v.pos[0]);
            xmax = xmax.max(v.pos[0]);
            zmin = zmin.min(v.pos[2]);
            zmax = zmax.max(v.pos[2]);
        }
        xmin < x && x < xmax && zmin < z && z < zmax
    };

    let carrier_top_covered = quads.iter().any(|q| {
        shade_idx(&q[0]) == 0
            && q.iter().all(|v| (v.pos[1] - 1.0).abs() < 1e-3)
            && covers(q, 8.5, 8.5)
    });
    assert!(
        !carrier_top_covered,
        "the block under a snow layer must not emit its covered top face"
    );

    let snow_top = quads.iter().any(|q| {
        shade_idx(&q[0]) == 0
            && q.iter()
                .all(|v| (v.pos[1] - (1.0 + 1.0 / 16.0)).abs() < 1e-3)
            && covers(q, 8.5, 8.5)
    });
    assert!(
        snow_top,
        "the snow layer's own top face must keep rendering"
    );

    let open_top_covered = quads.iter().any(|q| {
        shade_idx(&q[0]) == 0
            && q.iter().all(|v| (v.pos[1] - 1.0).abs() < 1e-3)
            && covers(q, 2.5, 2.5)
    });
    assert!(open_top_covered, "uncovered floor tops must still render");
}

/// A bottom slab has to seal the face under it, even though it's not opaque. Flip it to the top
/// half and it shouldn't. The cull only looks at the resolved boxes.
#[test]
fn any_floor_flush_neighbour_seals_the_face_beneath_it() {
    let carrier_top_drawn = |slot: usize| {
        let mut section = floor_section(Block::Stone);
        section.set_block(8, 1, 8, Block::StoneSlab);
        section.set_slab_state(
            8,
            1,
            8,
            SlabState::single(
                petramond_world::block_state::SlabSplit::Y,
                slot,
                Block::StoneSlab,
            ),
        );
        mesh(&section).opaque.chunks_exact(4).any(|q| {
            let span = |a: usize| {
                q.iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v.pos[a]), hi.max(v.pos[a]))
                    })
            };
            let (x0, x1) = span(0);
            let (z0, z1) = span(2);
            shade_idx(&q[0]) == 0
                && q.iter().all(|v| (v.pos[1] - 1.0).abs() < 1e-3)
                && x0 < 8.5
                && 8.5 < x1
                && z0 < 8.5
                && 8.5 < z1
        })
    };
    assert!(
        !carrier_top_drawn(0),
        "a bottom slab's base seals the carrier's top face"
    );
    assert!(
        carrier_top_drawn(1),
        "a TOP slab leaves the carrier's top face exposed"
    );
}

/// Ground decoration beside a snow layer is drawn STANDING IN the blanket its
/// cell displaced, and the blanket is drawn exactly once.
///
/// Worldgen gives a column one cover cell, so a pebble in a snowfield takes the
/// snow layer's place. The bed restores the snow blanket under the litter.
/// Half the litter boxes are exactly one texel tall, so
/// their top face lands on the blanket's own plane, which is why the bed joins
/// the decoration's box set instead of being emitted beside it: one set lets
/// the emitter's coincidence tie-break pick a winner, two sets would draw both
/// and z-fight. That is the part worth guarding.
#[test]
fn decoration_beside_snow_is_bedded_in_it_without_doubling_the_surface() {
    let coincident = (3.5 / 16.0, 10.5 / 16.0);
    let bare = (14.0 / 16.0, 2.0 / 16.0);

    let tops_at = |neighbour: Option<Block>, at: (f32, f32)| {
        let mut section = floor_section(Block::Grass);
        section.set_block(8, 1, 8, Block::PebblesSmall);
        if let Some(b) = neighbour {
            section.set_block(9, 1, 8, b);
        }
        mesh(&section)
            .opaque
            .chunks_exact(4)
            .filter(|q| {
                let span = |a: usize| {
                    q.iter()
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                            (lo.min(v.pos[a]), hi.max(v.pos[a]))
                        })
                };
                let (x0, x1) = span(0);
                let (z0, z1) = span(2);
                let (px, pz) = (8.0 + at.0, 8.0 + at.1);
                shade_idx(&q[0]) == 0
                    && q.iter()
                        .all(|v| (v.pos[1] - (1.0 + 1.0 / 16.0)).abs() < 1e-3)
                    && x0 < px
                    && px < x1
                    && z0 < pz
                    && pz < z1
            })
            .count()
    };

    assert_eq!(tops_at(None, coincident), 1, "the pebble's own box top");
    assert_eq!(tops_at(None, bare), 0, "no blanket without snow beside it");

    assert_eq!(
        tops_at(Some(Block::SnowLayer), bare),
        1,
        "the bed's surface"
    );
    assert_eq!(
        tops_at(Some(Block::SnowLayer), coincident),
        1,
        "the blanket and a coplanar litter box must not both draw the plane"
    );
}

/// A bedded cell wears its blanket for the block BELOW it too: the grass keeps
/// the snowy sides its undecorated neighbours have, and stops drawing the top
/// face the blanket now hides (which would otherwise z-fight it from far off,
/// the artifact `a_full_cube_under_a_snow_layer_culls_its_top_face` exists to
/// prevent — the two planes are 1/16 apart).
#[test]
fn a_bedded_cell_covers_the_grass_below_it_like_the_snow_it_stands_in() {
    let carrier_top_drawn = |neighbour: Option<Block>| {
        let mut section = floor_section(Block::Grass);
        section.set_block(8, 1, 8, Block::Fern);
        if let Some(b) = neighbour {
            section.set_block(9, 1, 8, b);
        }
        mesh(&section).opaque.chunks_exact(4).any(|q| {
            let span = |a: usize| {
                q.iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v.pos[a]), hi.max(v.pos[a]))
                    })
            };
            let (x0, x1) = span(0);
            let (z0, z1) = span(2);
            shade_idx(&q[0]) == 0
                && q.iter().all(|v| (v.pos[1] - 1.0).abs() < 1e-3)
                && x0 < 8.5
                && 8.5 < x1
                && z0 < 8.5
                && 8.5 < z1
        })
    };

    assert!(
        carrier_top_drawn(None),
        "a fern on bare grass leaves the grass top exposed"
    );
    assert!(
        !carrier_top_drawn(Some(Block::SnowLayer)),
        "a bedded fern's blanket seals the grass top beneath it"
    );
}

#[test]
fn a_snow_layer_casts_no_ao_onto_the_ground_beside_it() {
    let m = mesh(&section_with(&[
        ((7, 7, 8), Block::Stone),
        ((8, 7, 8), Block::Stone),
        ((8, 8, 8), Block::SnowLayer),
    ]));
    let open_floor_top: Vec<_> = m
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
        !open_floor_top.is_empty(),
        "the uncovered floor top renders"
    );
    assert!(
        open_floor_top.iter().all(|v| ao_idx(v) == 3),
        "floor corners against the snow layer stay open"
    );
}
