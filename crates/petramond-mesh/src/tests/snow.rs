use super::*;

/// A lowered cube's full 1×1 base is flush with the cell floor, so the full
/// block beneath it must CULL its top face — the two nearly-coplanar planes
/// (carrier top at y, snow top at y+1/16) z-fight from far above otherwise.
/// The lowered cube itself keeps rendering (its sunken top is inside the cell).
#[test]
fn a_full_cube_under_a_snow_layer_culls_its_top_face() {
    let mut section = floor_section(Block::Stone);
    section.set_block(8, 1, 8, Block::SnowLayer);
    let m = mesh(&section);

    // Every opaque emitter here pushes 4-vertex quads; group and classify.
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

    // No floor-top quad (y=1, +Y shade) may cover the snow-carrying cell.
    let carrier_top_covered = quads.iter().any(|q| {
        shade_idx(&q[0]) == 0
            && q.iter().all(|v| (v.pos[1] - 1.0).abs() < 1e-3)
            && covers(q, 8.5, 8.5)
    });
    assert!(
        !carrier_top_covered,
        "the block under a snow layer must not emit its covered top face"
    );

    // The snow layer's own sunken top (y = 1 + 1/16) still renders.
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

    // An uncovered floor cell still has its top drawn (the cull is per cell).
    let open_top_covered = quads.iter().any(|q| {
        shade_idx(&q[0]) == 0
            && q.iter().all(|v| (v.pos[1] - 1.0).abs() < 1e-3)
            && covers(q, 2.5, 2.5)
    });
    assert!(open_top_covered, "uncovered floor tops must still render");
}

/// The covers-below cull is GEOMETRIC, not family-keyed: whatever resolves a
/// box flush on its own floor that covers the whole cell footprint seals the
/// face beneath it. A bottom slab is neither opaque nor a lowered cube and
/// must seal; the same slab flipped to the TOP half must not — proving the
/// answer is read off the cell's resolved boxes and not off its row.
#[test]
fn any_floor_flush_neighbour_seals_the_face_beneath_it() {
    let carrier_top_drawn = |slot: usize| {
        let mut section = floor_section(Block::Stone);
        section.set_block(8, 1, 8, Block::StoneSlab);
        section.set_slab_state(
            8,
            1,
            8,
            SlabState::single(petramond_world::block_state::SlabSplit::Y, slot, Block::StoneSlab),
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
/// snow layer's place and used to leave a bare green hole. The bed puts the
/// blanket back — but half the litter boxes are exactly one texel tall, so
/// their top face lands on the blanket's own plane, which is why the bed joins
/// the decoration's box set instead of being emitted beside it: one set lets
/// the emitter's coincidence tie-break pick a winner, two sets would draw both
/// and z-fight. That is the part worth guarding.
#[test]
fn decoration_beside_snow_is_bedded_in_it_without_doubling_the_surface() {
    // Litter with a one-texel-tall box: `pebbles_small` puts one at
    // x 2..5, z 9..12, whose top is coplanar with the bed's.
    let coincident = (3.5 / 16.0, 10.5 / 16.0);
    // A corner of the same cell that no pebble box reaches.
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

    // No snow beside it: the pebble is bare ground, and only its own one-texel
    // box reaches the blanket's height.
    assert_eq!(tops_at(None, coincident), 1, "the pebble's own box top");
    assert_eq!(tops_at(None, bare), 0, "no blanket without snow beside it");

    // Snow beside it: the blanket appears across the cell, and the plane the
    // pebble already owned is still drawn exactly once.
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
