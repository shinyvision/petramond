use super::*;

/// Open columns are full sky (15 = 30 on the x2 scale), and nothing exceeds it.
#[test]
fn skylight_open_column_is_full() {
    let mut c = Chunk::new(0, 0);
    fill_chunk_layers(&mut c, 0..=0, Block::Stone);
    let sky = solo_skylight(&c);
    // Air directly above the floor, open to the sky -> full light.
    assert_eq!(sky.at(8, 1, 8), SKY_FULL);
    // Nothing ever exceeds full sky.
    assert!(sky.band.iter().all(|&v| v <= SKY_FULL));
}

/// Regression for dug shafts: removing the top block of a column must lower the
/// heightmap, otherwise the skylight band stops near the old surface and the
/// deeper open shaft abruptly turns black.
#[test]
fn skylight_dug_vertical_shaft_stays_lit_below_old_surface_margin() {
    let mut c = Chunk::new(0, 0);
    fill_chunk_layers(&mut c, 0..=80, Block::Stone);
    for y in (1..=80).rev() {
        c.set_block(8, y, 8, Block::Air);
    }

    assert_eq!(c.surface_y(8, 8), 0, "dug column heightmap lowered");
    let sky = solo_skylight(&c);
    assert_eq!(sky.at(8, 55, 8), SKY_FULL);
    assert_eq!(sky.at(8, 1, 8), SKY_FULL);
}

/// A sealed horizontal tunnel off an open vertical shaft: light falls off by
/// `-1/block` (= -2 on the x2 scale) into the tunnel -- the gradient the
/// feature is built on. Fully enclosed in stone so the open apron of a
/// standalone chunk can't leak light in and flatten it.
#[test]
fn skylight_tunnel_falls_off_by_one_per_block() {
    let mut c = Chunk::new(0, 0);
    // Solid stone slab y=0..=6 across the whole chunk.
    fill_chunk_layers(&mut c, 0..=6, Block::Stone);
    // Vertical shaft open to the sky at (8,*,8).
    for y in 1..=6 {
        c.set_block(8, y, 8, Block::Air);
    }
    // Horizontal tunnel at y=3 running +x off the shaft.
    for x in 9..=13 {
        c.set_block(x, 3, 8, Block::Air);
    }
    let sky = solo_skylight(&c);
    assert_eq!(sky.at(8, 3, 8), SKY_FULL, "open shaft is full sky");
    // Each air block into the tunnel costs 2 on the x2 scale (= 1 real).
    assert_eq!(sky.at(9, 3, 8), SKY_FULL - 2);
    assert_eq!(sky.at(10, 3, 8), SKY_FULL - 4);
    assert_eq!(sky.at(11, 3, 8), SKY_FULL - 6);
    // Monotonically darker deeper in.
    assert!(sky.at(13, 3, 8) < sky.at(9, 3, 8));
}

#[test]
fn skylight_flood_crosses_loaded_chunk_border() {
    let mut west = Chunk::new(0, 0);
    let mut east = Chunk::new(1, 0);
    for c in [&mut west, &mut east] {
        fill_chunk_layers(c, 0..=6, Block::Stone);
    }

    // The light source is an open shaft at the east edge of the west chunk.
    for y in 1..=6 {
        west.set_block(CHUNK_SX - 1, y, 8, Block::Air);
    }
    // The tunnel starts in the east chunk, just across the chunk border.
    for x in 0..=4 {
        east.set_block(x, 3, 8, Block::Air);
    }

    let isolated = solo_skylight(&east);
    assert_eq!(
        isolated.at(0, 3, 8),
        0,
        "without neighbor reads the border tunnel has no local sky source"
    );

    let (band, ylo, yhi) = compute_chunk_skylight_with_neighbors(&east, |cx, cz| {
        if cx == west.cx && cz == west.cz {
            Some(&west)
        } else if cx == east.cx && cz == east.cz {
            Some(&east)
        } else {
            None
        }
    });
    let sky = TestSky { band, ylo, yhi };

    assert_eq!(sky.at(0, 3, 8), SKY_FULL - 2);
    assert_eq!(sky.at(1, 3, 8), SKY_FULL - 4);
    assert_eq!(sky.at(2, 3, 8), SKY_FULL - 6);
}

/// Straight-down attenuation through a filtering medium, per layer: water costs
/// a FULL level per layer (2 on the x2 scale, the same rate as air) so light
/// drops off quickly underwater, while leaves cost HALF (1 on the x2 scale =
/// 0.5 real) — light reaches deeper into a canopy than into water.
#[test]
fn skylight_water_and_leaves_attenuate_per_layer_at_their_rates() {
    for (fill, per_layer) in [(Block::Water, 2), (Block::OakLeaves, 1)] {
        let sky = solo_skylight(&walled_shaft(fill));
        assert_eq!(
            sky.at(8, 9, 8),
            SKY_FULL,
            "air above the {fill:?} shaft is full sky"
        );
        for layer in 1..=3u8 {
            assert_eq!(
                sky.at(8, 9 - layer as i32, 8),
                SKY_FULL - layer * per_layer,
                "{fill:?} attenuation {layer} layer(s) deep"
            );
        }
    }
}

/// Volumetric depth darkening: the air BELOW a leaf canopy keeps losing 0.5 a
/// level (1 on the x2 scale) per block of descent, not just at the leaf -- so it
/// gets darker the deeper you go under cover (and digging down stays dark, see
/// `skylight_digging_down_under_cover_keeps_darkening`).
#[test]
fn skylight_air_below_canopy_darkens_with_depth() {
    let mut c = floored_chunk();
    // A leaf roof at y=10 over the whole chunk; open air pocket y=5..=9 below.
    fill_chunk_layers(&mut c, 10..=10, Block::OakLeaves);
    let sky = solo_skylight(&c);
    assert_eq!(
        sky.at(8, 11, 8),
        SKY_FULL,
        "open air above the canopy is full sky"
    );
    assert_eq!(
        sky.at(8, 10, 8),
        SKY_FULL - 1,
        "the leaf itself drops half a level"
    );
    // Each AIR block below the leaf keeps draining the under-canopy rate (1/block).
    assert_eq!(sky.at(8, 9, 8), SKY_FULL - 2);
    assert_eq!(sky.at(8, 8, 8), SKY_FULL - 3);
    assert_eq!(sky.at(8, 7, 8), SKY_FULL - 4);
    assert_eq!(sky.at(8, 6, 8), SKY_FULL - 5);
    assert_eq!(sky.at(8, 5, 8), SKY_FULL - 6);
}

/// Water drains a full level per block both THROUGH the water and on into the
/// air pocket beneath it -- the deeper under water, the darker.
#[test]
fn skylight_under_water_darkens_with_depth() {
    let mut c = floored_chunk();
    // Water body y=6..=10 over the whole chunk; open air pocket at y=5.
    fill_chunk_layers(&mut c, 6..=10, Block::Water);
    let sky = solo_skylight(&c);
    assert_eq!(
        sky.at(8, 11, 8),
        SKY_FULL,
        "open air above the water is full sky"
    );
    assert_eq!(sky.at(8, 10, 8), SKY_FULL - 2); // first water -1 level
    assert_eq!(sky.at(8, 6, 8), SKY_FULL - 10); // 5 water blocks -> -5 levels
    assert_eq!(sky.at(8, 5, 8), SKY_FULL - 12); // air below water keeps -1/block
}

/// Digging straight down under cover keeps getting darker: a shaft carved all
/// the way through the floor under a leaf roof darkens monotonically to the
/// bottom (the reported "digging down doesn't drop below the surface" bug).
#[test]
fn skylight_digging_down_under_cover_keeps_darkening() {
    let mut c = floored_chunk();
    fill_chunk_layers(&mut c, 10..=10, Block::OakLeaves);
    for y in 0..=4 {
        c.set_block(8, y, 8, Block::Air);
    } // dig the floor out at (8,*,8)
    let sky = solo_skylight(&c);
    // Strictly darker each block down, from just under the leaf to the bottom.
    for y in 0..10 {
        assert!(
            sky.at(8, y, 8) < sky.at(8, y + 1, 8),
            "expected light at y={y} < y={}; got {} !< {}",
            y + 1,
            sky.at(8, y, 8),
            sky.at(8, y + 1, 8),
        );
    }
    assert_eq!(
        sky.at(8, 0, 8),
        SKY_FULL - 11,
        "bottom of the dug shaft is much darker"
    );
}

/// Regression for the reported bug: an open dug shaft beside a water body must
/// NOT flatten the water's depth gradient. Before the fix, horizontal bleed
/// from the always-bright shaft re-lit the adjacent water to a constant level;
/// the sky descent now freezes sky-lit cells so the gradient survives.
#[test]
fn skylight_depth_gradient_survives_adjacent_open_shaft() {
    let mut c = floored_chunk();
    fill_chunk_layers(&mut c, 6..=10, Block::Water);
    // Dig a 1-wide shaft straight through the water at (8,8): re-opens to sky.
    for y in 6..=10 {
        c.set_block(8, y, 8, Block::Air);
    }
    let sky = solo_skylight(&c);
    // The shaft itself genuinely has sky access -> full sky all the way down.
    assert_eq!(sky.at(8, 10, 8), SKY_FULL);
    assert_eq!(sky.at(8, 6, 8), SKY_FULL);
    // The water column right next to it still darkens with depth (not flat).
    let col: Vec<u8> = (6..=10).rev().map(|y| sky.at(9, y, 8)).collect();
    assert_eq!(
        col,
        vec![
            SKY_FULL - 2,
            SKY_FULL - 4,
            SKY_FULL - 6,
            SKY_FULL - 8,
            SKY_FULL - 10,
        ]
    );
}

/// Leaf-covered light bleeds from an adjacent open skylight the same way
/// opaque-covered light does, but loses half as much light per covered step.
#[test]
fn skylight_leaf_covered_side_bleed_is_half_opaque_falloff() {
    let opaque = solo_skylight(&roof_with_open_shaft(Block::Stone));
    let leaf = solo_skylight(&roof_with_open_shaft(Block::OakLeaves));

    assert_eq!(opaque.at(8, 5, 8), SKY_FULL);
    assert_eq!(leaf.at(8, 5, 8), SKY_FULL);

    for dx in 1..=4 {
        let x = 8 + dx;
        let dx = dx as u8;
        assert_eq!(opaque.at(x, 5, 8), SKY_FULL - dx * 2);
        assert_eq!(leaf.at(x, 5, 8), SKY_FULL - dx);
    }
}
