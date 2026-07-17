use super::*;

/// A water source cell at the section's +X face, meshed with the east
/// neighbour either loaded (air) or not yet streamed in.
fn edge_water_mesh(east_section_loaded: bool) -> ChunkMesh {
    let mut section = Section::new(0, 0, 0);
    section.set_water(SECTION_SIZE - 1, 8, 8, Block::Water, 0);
    mesh_with(
        &section,
        |_, _, _| SKY_FULL,
        |wx, _, _| wx < SECTION_SIZE as i32 || east_section_loaded,
    )
}

#[test]
fn water_side_faces_at_unloaded_streaming_edges_are_culled() {
    let loaded_air = edge_water_mesh(true);
    let unloaded = edge_water_mesh(false);

    assert_eq!(
        loaded_air.transparent.len(),
        24,
        "loaded neighbour air keeps all water faces visible"
    );
    assert_eq!(
        unloaded.transparent.len(),
        20,
        "unloaded neighbour culls only the streaming-edge water side"
    );
    // The TOP face is emitted in both windings (back-face-culled transparent pass
    // keeps the surface visible from below), so each adds 6 extra indices.
    assert_eq!(loaded_air.transparent_idx.len(), 36 + 6);
    assert_eq!(unloaded.transparent_idx.len(), 30 + 6);
}

/// Still water uses the still tile and renders at the (recessed) full height;
/// flowing water uses the animated flow tile and renders lower, so the surface
/// slopes. Locks the mesher's water tile selection + variable-height geometry.
#[test]
fn water_meshing_picks_still_vs_flow_tiles_and_varies_height() {
    use crate::atlas::Tile;

    let still_id = Tile::named("water_still").index() as u32;
    let flow_id = Tile::named("water_flow").index() as u32;

    // A 5x5 pool of sources on a stone floor at y=4, plus one explicitly
    // flowing cell (falloff 4) at the east rim that opens onto air.
    let mut section = Section::new(0, 0, 0);
    for z in 6..=10 {
        for x in 6..=10 {
            section.set_block(x, 4, z, Block::Stone);
            section.set_water(x, 5, z, Block::Water, 0); // source
        }
    }
    section.set_block(11, 4, 8, Block::Stone);
    section.set_water(11, 5, 8, Block::Water, 4); // flowing, opens east onto air

    let m = mesh(&section);

    // Decode the upward-facing tile for each top vertex (those raised above the
    // cell floor). Collect tile ids and the lowest/highest surface heights.
    let mut saw_still = false;
    let mut saw_flow = false;
    let mut min_top = f32::INFINITY;
    let mut max_top: f32 = 0.0;
    for v in &m.transparent {
        let tile = tile_idx(v);
        // Top vertices sit above the cell base (y=5); skip the side/bottom ones.
        if v.pos[1] > 5.05 {
            min_top = min_top.min(v.pos[1]);
            max_top = max_top.max(v.pos[1]);
        }
        if tile == still_id {
            saw_still = true;
        } else if tile == flow_id {
            saw_flow = true;
        }
    }

    assert!(
        saw_still,
        "interior still sources should use the still tile"
    );
    assert!(saw_flow, "the flowing rim cell should use the flow tile");
    // Full sources sit at the recessed 0.875; the falloff-4 cell is well below.
    assert!(
        max_top <= 5.9,
        "water tops are recessed below the full block (got {max_top})"
    );
    assert!(
        min_top < 5.6,
        "the flowing cell should slope notably lower than a source (got {min_top})"
    );
}

/// A submerged (capped) water cell must render its side face toward a shorter
/// open-surface neighbour — the exposed vertical step — so a 1-deep flow stepping
/// down doesn't show the floor through the height gap.
#[test]
fn submerged_water_renders_exposed_step_toward_a_shorter_neighbour() {
    let mut section = section_with(&[((8, 3, 8), Block::Stone), ((9, 3, 8), Block::Stone)]);
    // 2-deep column at x=8 -> the y=4 cell is capped (water above) and full.
    section.set_water(8, 4, 8, Block::Water, 0);
    section.set_water(8, 5, 8, Block::Water, 0);
    // Shorter open-surface flowing cell next door (air above it).
    section.set_water(9, 4, 8, Block::Water, 3);

    let m = mesh(&section);

    // The capped cell's east face lives on the x=9 plane. It is rendered (not
    // culled water<->water) as a BAND: trimmed at the bottom to the neighbour's
    // recessed surface (~0.79 here) and full at the top, so the submerged part
    // (water behind water) isn't drawn. The trimmed bottom edge is the only water
    // vertex on that plane strictly inside (4, 5); a culled or full-height face
    // would have none there.
    let band_bottom = m.transparent.iter().any(|v| {
        (v.pos[0] - 9.0).abs() < 1e-3 && shade_idx(v) == 2 && v.pos[1] > 4.05 && v.pos[1] < 4.95
    });
    assert!(
        band_bottom,
        "submerged cell must render its exposed step as a trimmed band above the neighbour"
    );
}

/// Falling water is also full-height. When it diverges beside a thinner
/// same-level flow, its internal side must render the exposed step; otherwise the
/// two water meshes do not meet and the floor/terrain shows through as a wedge.
#[test]
fn falling_water_renders_exposed_step_toward_a_shorter_neighbour() {
    const FALLING_META: u8 = 0x80;

    let mut section = section_with(&[((8, 3, 8), Block::Stone), ((9, 3, 8), Block::Stone)]);
    section.set_water(8, 4, 8, Block::Water, FALLING_META);
    section.set_water(9, 4, 8, Block::Water, 3);

    let m = mesh(&section);

    let step = m
        .transparent
        .iter()
        .filter(|v| (v.pos[0] - 9.0).abs() < 1e-3 && shade_idx(v) == 2)
        .collect::<Vec<_>>();

    assert!(
        step.iter().any(|v| (v.pos[1] - 5.0).abs() < 1e-3),
        "falling water step must reach the full cell top"
    );
    assert!(
        step.iter().any(|v| v.pos[1] > 4.05 && v.pos[1] < 4.95),
        "falling water step must be trimmed to the neighbour's lower surface"
    );
}

/// The frozen-sea sheet — an ice layer capping water at the section's top row
/// — must emit its geometry into the TRANSPARENT (alpha-blended) buffer: ice
/// is a translucent block, and a routing regression in either direction is
/// invisible or wrongly-solid ice.
#[test]
fn sea_ice_sheet_over_water_emits_translucent_geometry() {
    let mut section = Section::new(0, 0, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            section.set_block(x, 0, z, Block::Stone);
            for y in 1..15 {
                section.set_block(x, y, z, Block::Water);
            }
            section.set_block(x, 15, z, Block::Ice);
        }
    }
    let m = mesh(&section);
    let ice_tile = Block::Ice.tiles()[0].index() as u32;
    let blended = m
        .translucent
        .iter()
        .filter(|v| tile_idx(v) == ice_tile)
        .count();
    let cutout = m.opaque.iter().filter(|v| tile_idx(v) == ice_tile).count();
    let water_pass = m
        .transparent
        .iter()
        .filter(|v| tile_idx(v) == ice_tile)
        .count();
    assert!(blended > 0, "the ice sheet must emit translucent-pass geometry");
    assert_eq!(cutout, 0, "no ice face may leak into the cutout pass");
    assert_eq!(water_pass, 0, "no ice face may leak into the water pass");

    // Water under ice looks like water under ANY block: the ordinary recessed
    // 8/9 source surface, never pulled flush to the ice underside. (Flush
    // sealing under ice was tried and reverted — consistency won.)
    let top_water = 14.0 + 8.0 / 9.0;
    assert!(
        m.transparent
            .iter()
            .any(|v| (v.pos[1] - top_water).abs() < 1e-4),
        "water under the sheet keeps the ordinary recessed source surface"
    );
    assert!(
        !m.transparent.iter().any(|v| v.pos[1] == 15.0),
        "water must not press flush against the ice underside"
    );
}

/// Water never climbs to meet a block above it — ANY block: a FLOWING
/// trickle under a stone bridge keeps its own recessed surface, a STILL
/// SOURCE under stone keeps the classic 8/9 gap, and a source under ICE
/// keeps the exact same gap (flush "sealing" lids were tried in three
/// variants and all reverted — `world::water::fills_cell`).
#[test]
fn water_under_ordinary_blocks_keeps_its_own_surface() {
    let mut section = Section::new(0, 0, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            section.set_block(x, 0, z, Block::Stone);
        }
    }
    // A flowing cell (level 4 → amount 4 → surface 4/9) and a still SOURCE,
    // each under a stone "bridge" block; a source under ICE must match stone.
    section.set_water(4, 1, 8, Block::Water, 4);
    section.set_block(4, 2, 8, Block::Stone);
    section.set_water(8, 1, 8, Block::Water, 0);
    section.set_block(8, 2, 8, Block::Stone);
    section.set_water(12, 1, 8, Block::Water, 0);
    section.set_block(12, 2, 8, Block::Ice);

    let m = mesh(&section);
    let in_col = |v: &&Vertex, x: usize| v.pos[0] >= x as f32 && v.pos[0] <= x as f32 + 1.0;
    let flowing_top = 1.0 + 4.0 / 9.0;
    let source_top = 1.0 + 8.0 / 9.0;
    assert!(
        m.transparent
            .iter()
            .any(|v| in_col(&v, 4) && (v.pos[1] - flowing_top).abs() < 1e-4),
        "the flowing trickle keeps its 4/9 surface under the bridge"
    );
    assert!(
        m.transparent
            .iter()
            .any(|v| in_col(&v, 8) && (v.pos[1] - source_top).abs() < 1e-4),
        "a still source under ordinary stone keeps the 8/9 gap"
    );
    assert!(
        m.transparent
            .iter()
            .any(|v| in_col(&v, 12) && (v.pos[1] - source_top).abs() < 1e-4),
        "a still source under ice keeps the exact same 8/9 gap"
    );
    assert!(
        !m.transparent.iter().any(|v| in_col(&v, 12) && v.pos[1] == 2.0),
        "no flush sealing under ice"
    );
}

/// Still sources never wear the FLOW look, whatever sits in the water: a
/// block submerged in a still sea makes the recessed cell under/around it
/// slope against its full mid-column neighbours, but two adjacent still
/// sources never flow into each other — so no flow tile and no flow heading
/// anywhere (`water::surface_flow_dir`). A genuinely FLOWING cell beside
/// sources keeps its flow look.
#[test]
fn blocks_sitting_in_still_water_grow_no_flow_streaks() {
    let flow_tile = crate::atlas::engine().water_flow.index() as u32;

    // A still walled pool (all sources, meta 0) with a block resting in it —
    // walled because out-of-section reads are air, which would otherwise put
    // a genuine waterfall edge on the section border.
    let mut sea = Section::new(0, 0, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            let rim = x == 0 || z == 0 || x == SECTION_SIZE - 1 || z == SECTION_SIZE - 1;
            sea.set_block(x, 0, z, Block::Stone);
            for y in 1..4 {
                if rim {
                    sea.set_block(x, y, z, Block::Stone);
                } else {
                    sea.set_water(x, y, z, Block::Water, 0);
                }
            }
        }
    }
    sea.set_block(8, 3, 8, Block::Stone); // at the surface row
    sea.set_block(8, 2, 8, Block::Stone); // fully submerged
    let m = mesh(&sea);
    assert!(
        !m.transparent.iter().any(|v| tile_idx(v) == flow_tile),
        "still sources around a submerged block must not render as flowing"
    );

    // Contrast: one genuinely flowing cell in the open keeps the flow tile.
    let mut stream = Section::new(0, 0, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            stream.set_block(x, 0, z, Block::Stone);
        }
    }
    stream.set_water(8, 1, 8, Block::Water, 0);
    stream.set_water(9, 1, 8, Block::Water, 3); // flowing neighbour
    let m = mesh(&stream);
    assert!(
        m.transparent.iter().any(|v| tile_idx(v) == flow_tile),
        "genuinely flowing water keeps its flow look"
    );
}
