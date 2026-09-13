use super::*;

/// A water source cell at the section's +X face, meshed with the east
/// neighbour either loaded (air) or not yet streamed in.
fn edge_water_mesh(east_section_loaded: bool) -> ChunkMesh {
    let mut section = Section::new(0, 0, 0);
    section.set_fluid(SECTION_SIZE - 1, 8, 8, Block::Water, 0);
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

    // The TOP face rides its own cull-none stream (it must stay visible from
    // below), so the back-face-culled stream carries the other five.
    assert_eq!(
        loaded_air.transparent.len() + loaded_air.transparent_two_sided.len(),
        24,
        "loaded neighbour air keeps all water faces visible"
    );
    assert_eq!(
        unloaded.transparent.len() + unloaded.transparent_two_sided.len(),
        20,
        "unloaded neighbour culls only the streaming-edge water side"
    );
    assert_eq!(loaded_air.transparent_two_sided.len(), 4, "one top face");
    assert_eq!(unloaded.transparent_two_sided.len(), 4, "one top face");
}

/// Still water uses the still tile and renders at the (recessed) full height;
/// flowing water uses the animated flow tile and renders lower, so the surface
/// slopes. Locks the mesher's water tile selection + variable-height geometry.
#[test]
fn water_meshing_picks_still_vs_flow_tiles_and_varies_height() {
    let still_id = Block::Water.fluid_still_tile().index() as u32;
    let flow_id = Block::Water.fluid_flow_tile().index() as u32;

    // A 5x5 pool of sources on a stone floor at y=4, plus one explicitly
    // flowing cell (falloff 4) at the east rim that opens onto air.
    let mut section = Section::new(0, 0, 0);
    for z in 6..=10 {
        for x in 6..=10 {
            section.set_block(x, 4, z, Block::Stone);
            section.set_fluid(x, 5, z, Block::Water, 0); // source
        }
    }
    section.set_block(11, 4, 8, Block::Stone);
    section.set_fluid(11, 5, 8, Block::Water, 4); // flowing, opens east onto air

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
    section.set_fluid(8, 4, 8, Block::Water, 0);
    section.set_fluid(8, 5, 8, Block::Water, 0);
    // Shorter open-surface flowing cell next door (air above it).
    section.set_fluid(9, 4, 8, Block::Water, 3);

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
    section.set_fluid(8, 4, 8, Block::Water, FALLING_META);
    section.set_fluid(9, 4, 8, Block::Water, 3);

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
    assert!(
        blended > 0,
        "the ice sheet must emit translucent-pass geometry"
    );
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
/// variants and all reverted — `world::fluid::fills_cell`).
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
    section.set_fluid(4, 1, 8, Block::Water, 4);
    section.set_block(4, 2, 8, Block::Stone);
    section.set_fluid(8, 1, 8, Block::Water, 0);
    section.set_block(8, 2, 8, Block::Stone);
    section.set_fluid(12, 1, 8, Block::Water, 0);
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
        !m.transparent
            .iter()
            .any(|v| in_col(&v, 12) && v.pos[1] == 2.0),
        "no flush sealing under ice"
    );
}

/// Still sources never wear the FLOW look, whatever sits in the water: a
/// block submerged in a still sea makes the recessed cell under/around it
/// slope against its full mid-column neighbours, but two adjacent still
/// sources never flow into each other — so no flow tile and no flow heading
/// anywhere (`fluid_math::surface_flow_dir`). A genuinely FLOWING cell beside
/// sources keeps its flow look.
#[test]
fn blocks_sitting_in_still_water_grow_no_flow_streaks() {
    let flow_tile = Block::Water.fluid_flow_tile().index() as u32;

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
                    sea.set_fluid(x, y, z, Block::Water, 0);
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
    stream.set_fluid(8, 1, 8, Block::Water, 0);
    stream.set_fluid(9, 1, 8, Block::Water, 3); // flowing neighbour
    let m = mesh(&stream);
    assert!(
        m.transparent.iter().any(|v| tile_idx(v) == flow_tile),
        "genuinely flowing water keeps its flow look"
    );
}

/// Every registered fluid — whatever its row says — is under test here.
fn fluids() -> impl Iterator<Item = Block> {
    petramond_world::fluid::medium::media().iter().copied()
}

pub(super) fn medium_slot(v: &Vertex) -> u32 {
    (v.packed2 >> crate::vertex::FLUID_MEDIUM_SHIFT) & crate::vertex::FLUID_MEDIUM_MASK
}

fn shows_flow_strip(v: &Vertex) -> bool {
    v.packed2 & crate::vertex::FLUID_FLOW_FLAG2 != 0
}

pub(super) fn face_of(v: &Vertex) -> crate::face::Face {
    use crate::vertex::{NORMAL_CODE_MASK, NORMAL_CODE_SHIFT};
    let code = (v.packed2 >> NORMAL_CODE_SHIFT) & NORMAL_CODE_MASK;
    crate::face::Face::ALL
        .into_iter()
        .find(|f| f.normal_code() == code)
        .unwrap_or_else(|| panic!("fluid vertex with normal code {code}"))
}

/// The quads carrying `fluid`'s medium in their vertex lane, from every
/// stream — the lane, not the stream, is what makes a face a fluid face.
pub(super) fn fluid_quads(m: &ChunkMesh, fluid: Block) -> Vec<[Vertex; 4]> {
    let slot = petramond_world::fluid::medium::medium_index(fluid).expect("a fluid row") + 1;
    [&m.opaque, &m.transparent, &m.transparent_two_sided]
        .into_iter()
        .flat_map(|stream| stream.chunks_exact(4))
        .filter(|q| medium_slot(&q[0]) == slot)
        .map(|q| [q[0], q[1], q[2], q[3]])
        .collect()
}

pub(super) fn geometric_normal(q: &[Vertex; 4]) -> [f32; 3] {
    let [p0, p1, p2] = [q[0].pos, q[1].pos, q[2].pos];
    let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
    let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
    [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ]
}

/// Every fluid meshes from its OWN row through the one fluid path: each face
/// carries its medium index in the vertex lane (every lane-bearing vertex is
/// one of its faces), still faces draw the row's still tile and flowing ones
/// its flow tile with the flow-strip flag, and the medium's opacity picks the
/// stream — an opaque body draws with the solid terrain, its top in both
/// windings so it reads from underneath; a translucent one rides the blended
/// streams only.
#[test]
fn every_fluid_meshes_from_its_own_row() {
    for fluid in fluids() {
        assert_meshes_from_its_row(fluid);
    }
}

/// Every fluid meshes from its own row (see `every_fluid_meshes_from_its_own_row`).
pub(super) fn assert_meshes_from_its_row(fluid: Block) {
    use crate::face::Face;

    let opaque = fluid.fluid_def().expect("a fluid row").medium.is_opaque();
    let still = fluid.fluid_still_tile().index() as u32;
    let flow = fluid.fluid_flow_tile().index() as u32;

    let mut section = Section::new(0, 0, 0);
    for z in 6..=10 {
        for x in 6..=10 {
            section.set_block(x, 4, z, Block::Stone);
            section.set_fluid(x, 5, z, fluid, 0); // source
        }
    }
    section.set_block(11, 4, 8, Block::Stone);
    section.set_fluid(11, 5, 8, fluid, 4); // flowing rim, opens east
    let m = mesh(&section);

    let quads = fluid_quads(&m, fluid);
    let laned = [&m.opaque, &m.transparent, &m.transparent_two_sided]
        .into_iter()
        .flatten()
        .filter(|v| medium_slot(v) != 0)
        .count();
    assert_eq!(
        laned,
        quads.len() * 4,
        "{fluid:?}: a face carries another medium"
    );

    let tops: Vec<_> = quads
        .iter()
        .filter(|q| face_of(&q[0]) == Face::PosY)
        .collect();
    let sides: Vec<_> = quads
        .iter()
        .filter(|q| {
            matches!(
                face_of(&q[0]),
                Face::PosX | Face::NegX | Face::PosZ | Face::NegZ
            )
        })
        .collect();
    assert!(
        tops.iter()
            .any(|q| tile_idx(&q[0]) == still && !shows_flow_strip(&q[0])),
        "{fluid:?}: still tops draw the row's still tile"
    );
    assert!(
        sides
            .iter()
            .any(|q| tile_idx(&q[0]) == still && !shows_flow_strip(&q[0])),
        "{fluid:?}: source walls draw the row's still tile"
    );
    assert!(
        sides
            .iter()
            .any(|q| tile_idx(&q[0]) == flow && shows_flow_strip(&q[0])),
        "{fluid:?}: the flowing rim streams on the row's flow tile"
    );
    assert!(
        quads
            .iter()
            .flatten()
            .all(|v| !shows_flow_strip(v) || tile_idx(v) == flow),
        "{fluid:?}: only flow-tile faces carry the flow-strip flag"
    );

    let in_opaque = m.opaque.iter().any(|v| medium_slot(v) != 0);
    let in_blended = m
        .transparent
        .iter()
        .chain(&m.transparent_two_sided)
        .any(|v| medium_slot(v) != 0);
    assert_eq!(
        (in_opaque, in_blended),
        (opaque, !opaque),
        "{fluid:?}: the medium's opacity picks the stream"
    );
    if opaque {
        let up = tops.iter().filter(|q| geometric_normal(q)[1] > 0.0).count();
        assert!(
            up > 0 && up * 2 == tops.len(),
            "{fluid:?}: every opaque top is emitted in both windings"
        );
    }
}

/// The face set of a fluid BODY, for every fluid: a free-standing two-deep
/// pool emits side walls only on its outer boundary planes, each declaring
/// the outward normal and wound front-facing along it, and no face at all
/// between two full same-fluid cells. Sides are back-face culled, so an
/// inward-wound or interior wall would show the far side of the body.
#[test]
fn fluid_body_emits_only_outward_boundary_walls() {
    use crate::face::Face;

    const LO: usize = 6;
    const HI: usize = 9; // exclusive
    for fluid in fluids() {
        let mut section = floor_section(Block::Stone);
        for z in LO..HI {
            for x in LO..HI {
                section.set_fluid(x, 1, z, fluid, 0);
                section.set_fluid(x, 2, z, fluid, 0);
            }
        }
        let m = mesh(&section);
        let quads = fluid_quads(&m, fluid);

        let mut walls_seen = [false; 4];
        for quad in quads.iter().filter(|q| face_of(&q[0]) != Face::PosY) {
            let face = face_of(&quad[0]);
            assert!(
                matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ),
                "{fluid:?}: a {face:?} face on a body resting on rock"
            );
            let (dx, _, dz) = face.dir();
            // The wall lies on the body's outer plane on its declared side.
            let (axis, plane) = if dx != 0 { (0, dx) } else { (2, dz) };
            let want = if plane > 0 { HI } else { LO } as f32;
            assert!(
                quad.iter().all(|v| (v.pos[axis] - want).abs() < 1e-4),
                "{fluid:?}: {face:?} wall off the body boundary: {:?}",
                quad.iter().map(|v| v.pos).collect::<Vec<_>>()
            );
            // Winding: the quad's geometric normal points along the declared
            // outward direction (front-facing under back-face culling).
            let n = geometric_normal(quad);
            assert!(
                n[0] * dx as f32 + n[2] * dz as f32 > 1e-6,
                "{fluid:?}: {face:?} wall wound inward (geometric normal {n:?})"
            );
            walls_seen[match face {
                Face::PosX => 0,
                Face::NegX => 1,
                Face::PosZ => 2,
                _ => 3,
            }] = true;
        }
        assert_eq!(
            walls_seen, [true; 4],
            "{fluid:?}: every outer boundary plane grows a wall"
        );
        // The surface sheet: only the open top layer, never the capped cells.
        let tops: Vec<_> = quads
            .iter()
            .filter(|q| face_of(&q[0]) == Face::PosY)
            .collect();
        assert!(
            !tops.is_empty() && tops.iter().flat_map(|q| q.iter()).all(|v| v.pos[1] > 2.5),
            "{fluid:?}: only the open top layer emits a surface"
        );
    }
}

/// A FALLING stream draws the flow tile on every exposed face, for every
/// fluid: the FALLING bit decides, not the horizontal surface gradient. A
/// column in open air has a symmetric neighbourhood (zero gradient), so its
/// exposed top — the topmost cell when the pour above is gone or not yet
/// streamed — must not fall back to the still tile. Sides of the still
/// SOURCE feeding a worldgen-style ceiling pour stay calm; the falling cells
/// under it stream.
#[test]
fn falling_fluid_column_draws_the_flow_tile_on_every_exposed_face() {
    use crate::face::Face;
    const FALLING_META: u8 = 0x80;

    for fluid in fluids() {
        let still_id = fluid.fluid_still_tile().index() as u32;
        let flow_id = fluid.fluid_flow_tile().index() as u32;
        let is_side = |q: &&[Vertex; 4]| face_of(&q[0]) != Face::PosY;

        // Ceiling pour: a still source under rock at y=7, falling cells 1..=6.
        let mut section = floor_section(Block::Stone);
        section.set_block(8, 8, 8, Block::Stone);
        section.set_fluid(8, 7, 8, fluid, 0);
        for y in 1..=6 {
            section.set_fluid(8, y, 8, fluid, FALLING_META);
        }
        let quads = fluid_quads(&mesh(&section), fluid);
        assert!(
            quads.iter().any(|q| is_side(&q)),
            "{fluid:?}: the column emits walls"
        );
        for quad in quads.iter().filter(is_side) {
            let base = quad.iter().map(|v| v.pos[1]).fold(f32::INFINITY, f32::min);
            let tiles: Vec<u32> = quad.iter().map(tile_idx).collect();
            let (want, streams) = if base < 6.5 {
                (flow_id, true)
            } else {
                (still_id, false)
            };
            assert!(
                tiles.iter().all(|&t| t == want)
                    && quad.iter().all(|v| shows_flow_strip(v) == streams),
                "{fluid:?}: wall at y={base} draws {tiles:?}, want {want} (flowing: {streams})"
            );
        }

        // Truncated fall: the same stream with nothing above its top cell.
        let mut section = floor_section(Block::Stone);
        for y in 1..=6 {
            section.set_fluid(8, y, 8, fluid, FALLING_META);
        }
        let quads = fluid_quads(&mesh(&section), fluid);
        let top_tiles: Vec<u32> = quads
            .iter()
            .filter(|q| !is_side(q))
            .map(|q| tile_idx(&q[0]))
            .collect();
        assert!(
            !top_tiles.is_empty() && top_tiles.iter().all(|&t| t == flow_id),
            "{fluid:?}: an exposed falling top draws {top_tiles:?}, want flow {flow_id}"
        );
        assert!(
            quads.iter().flatten().all(|v| tile_idx(v) == flow_id),
            "{fluid:?}: every falling wall draws the flow tile"
        );
    }
}

/// Two pockets in a cave (no skylight): a source sealed under a stone lid with
/// rock on every side, and a source in a one-wide trench whose walls rise past
/// its open top.
fn cave_pockets(block: Block) -> Section {
    let place = |section: &mut Section, x: usize, z: usize| {
        if block.is_fluid() {
            section.set_fluid(x, 1, z, block, 0);
        } else {
            section.set_block(x, 1, z, block);
        }
    };
    let mut section = floor_section(Block::Stone);
    place(&mut section, 4, 4);
    section.set_block(4, 2, 4, Block::Stone);
    for (x, z) in [(3, 4), (5, 4), (4, 3), (4, 5)] {
        section.set_block(x, 1, z, Block::Stone);
    }
    place(&mut section, 10, 10);
    for (x, z) in [(9, 10), (11, 10)] {
        section.set_block(x, 1, z, Block::Stone);
        section.set_block(x, 2, z, Block::Stone);
    }
    section
}

/// The flood's seeding around `block`: its own cell holds its full emission
/// and each face neighbour that accepts light one decay step of it; rock takes
/// none. Nothing else in the scene is lit.
fn emission_field(
    section: &Section,
    block: Block,
) -> impl Fn(i32, i32, i32) -> petramond_world::light::LightRgb + '_ {
    use petramond_world::light::LightRgb;
    let [er, eg, eb] = block.light_emission_rgb();
    let own = LightRgb::new(er, eg, eb);
    move |wx: i32, wy: i32, wz: i32| -> LightRgb {
        let at = |x: i32, y: i32, z: i32| {
            if in_section(x, y, z) {
                Block::from_id(section.block_raw(x as usize, y as usize, z as usize))
            } else {
                Block::Air
            }
        };
        if at(wx, wy, wz) == block {
            return own;
        }
        if at(wx, wy, wz).is_opaque() {
            return LightRgb::ZERO;
        }
        let beside = [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ]
        .into_iter()
        .any(|(dx, dy, dz)| at(wx + dx, wy + dy, wz + dz) == block);
        if beside {
            own.decayed()
        } else {
            LightRgb::ZERO
        }
    }
}

/// A fluid's faces are lit by its own emission by exactly its medium's
/// `self_lit` fraction, whatever the cell in front of them holds. The cube
/// path lights a face from the cell it fronts, and a recessed surface under a
/// lid fronts the lid — an opaque cell the flood never enters — while grid AO
/// reads the rock around a trench as shadow on the surface that IS the light
/// source. Checked for every fluid row: a fully self-lit emitter carries its
/// emission and open AO on every face; a fluid that lights none of itself
/// keeps the sampled darkness and its AO.
#[test]
fn a_fluid_lights_its_faces_by_its_medium_self_lit_fraction() {
    use crate::vertex::decode_vertex_light;
    use petramond_world::light::{BlockLight6, LightRgb};

    const AO_OPEN: u32 = 3;

    for fluid in fluids() {
        let fraction = fluid.fluid_def().expect("a fluid row").medium.self_lit;
        let [er, eg, eb] = fluid.light_emission_rgb();
        let emission = BlockLight6::from_x2(LightRgb::new(er, eg, eb)).channels();
        let section = cave_pockets(fluid);
        let m = mesh_lit(
            &section,
            |_, _, _| 0,
            emission_field(&section, fluid),
            |_, _, _| true,
        );
        let quads = fluid_quads(&m, fluid);
        let lidded_top = quads.iter().flatten().any(|v| {
            face_of(v) == crate::face::Face::PosY
                && (4.0..=5.0).contains(&v.pos[0])
                && (4.0..=5.0).contains(&v.pos[2])
        });
        assert!(
            lidded_top,
            "{fluid:?}: the recessed surface under the lid is drawn"
        );

        let verts: Vec<&Vertex> = quads.iter().flatten().collect();
        if fraction >= 1.0 {
            for v in &verts {
                let got = decode_vertex_light(v).channels();
                assert!(
                    (0..3).all(|i| got[i] >= emission[i]) && ao_idx(v) == AO_OPEN,
                    "{fluid:?}: face vertex at {:?} carries {got:?} / AO {}, own emission {emission:?}",
                    v.pos,
                    ao_idx(v)
                );
            }
        } else if fraction == 0.0 {
            assert!(
                verts.iter().any(|v| ao_idx(v) < AO_OPEN),
                "{fluid:?}: a fluid that lights none of itself keeps its AO in the trench"
            );
            if emission == [0; 3] {
                assert!(
                    verts.iter().all(|v| decode_vertex_light(v).is_dark()),
                    "{fluid:?}: an unlit, non-emissive fluid stays dark"
                );
            }
        }
    }
}

/// An emissive SOLID cube is not a self-lit face: its sides keep the light
/// sampled in front of them and their contact AO. Keying the lift on emission
/// alone made every glowing block glow flat on all six faces.
#[test]
fn an_emissive_solid_cube_keeps_its_sampled_light_and_ao() {
    let block = Block::all()
        .iter()
        .copied()
        .find(|b| {
            !b.is_fluid()
                && b.is_opaque()
                && b.shape_family() == petramond_world::block::ShapeFamily::Cube
                && b.light_emission_rgb() != [0; 3]
        })
        .expect("the regression needs an emissive solid cube row");
    let section = cave_pockets(block);
    let m = mesh_lit(
        &section,
        |_, _, _| 0,
        emission_field(&section, block),
        |_, _, _| true,
    );
    let tile_ids: Vec<u32> = block.tiles().iter().map(|t| t.index() as u32).collect();
    let faces: Vec<&Vertex> = m
        .opaque
        .iter()
        .filter(|v| (10.0..=11.0).contains(&v.pos[0]) && v.pos[1] > 1.5 && v.pos[1] <= 2.0)
        .filter(|v| tile_ids.contains(&tile_idx(v)))
        .collect();
    assert!(!faces.is_empty(), "the trench block's top is drawn");
    assert!(
        faces.iter().any(|v| ao_idx(v) < 3),
        "{block:?}: an emissive cube's top keeps the trench walls' AO"
    );
}
