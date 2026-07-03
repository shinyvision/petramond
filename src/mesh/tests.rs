use super::face::{should_flip, vertex_ao};
use super::*;
use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};

/// A cross-model plant adds a two-plane X billboard to the OPAQUE (cutout) pass,
/// drawn in both windings, and does NOT cull its supporting block's faces.
#[test]
fn cross_plant_emits_double_sided_billboards() {
    let air = |_: i32, _: i32, _: i32| 0u8;
    let biome0 = |_: i32, _: i32| 0u8;
    let light = |_: i32, _: i32, _: i32| SKY_FULL;

    // Bare stone cube at an interior voxel: all 6 faces drawn (air neighbours).
    let mut bare = Chunk::new(0, 0);
    bare.set_block_raw(8, 64, 8, Block::Stone.id());
    let m0 = build_mesh(&bare, air, biome0, light);
    assert_eq!(
        m0.opaque.len(),
        24,
        "interior stone cube should emit 6 quads"
    );

    // Same, plus a short-grass plant on top.
    let mut withplant = Chunk::new(0, 0);
    withplant.set_block_raw(8, 64, 8, Block::Stone.id());
    withplant.set_block_raw(8, 65, 8, Block::ShortGrass.id());
    let m1 = build_mesh(&withplant, air, biome0, light);

    // Plant adds exactly 2 planes x 4 verts = 8 verts, and 2 planes x (6 front +
    // 6 back) = 24 indices. The stone's faces are untouched (plant is non-opaque).
    assert_eq!(
        m1.opaque.len() - m0.opaque.len(),
        8,
        "plant should add 8 verts"
    );
    assert_eq!(
        m1.opaque_idx.len() - m0.opaque_idx.len(),
        24,
        "plant should add 24 indices (both windings)"
    );
    assert!(
        m1.transparent.is_empty(),
        "plant must not feed the alpha pass"
    );
}

/// Leaves must render in the OPAQUE pass, not the alpha-blended one. Proof: a
/// chunk that has leaves but NO water must produce an empty transparent buffer
/// (only water feeds it now) and a non-empty opaque buffer.
#[test]
fn leaves_go_to_opaque_pass() {
    let mut c = Chunk::new(0, 0);
    c.set_block_raw(8, 64, 8, Block::OakLeaves.id());

    let mesh = mesh_solo(&mut c);
    assert!(
        mesh.transparent_idx.is_empty(),
        "leaves+no-water chunk should have an empty transparent buffer"
    );
    assert!(
        !mesh.opaque_idx.is_empty(),
        "leaves should fill the opaque buffer"
    );
}

fn edge_water_mesh(east_chunk_loaded: bool) -> ChunkMesh {
    let mut chunk = Chunk::new(0, 0);
    chunk.set_block_raw(CHUNK_SX - 1, 8, 8, Block::Water.id());
    build_mesh_lods_with_loaded_neighbors(
        &chunk,
        |_, _, _| 0u8,
        |_, _, _| 0u8,
        |_, _| 0u8,
        |_, _, _| SKY_FULL,
        |_, _, _| 0u8,
        |cx, cz| cx == 0 && cz == 0 || east_chunk_loaded && cx == 1 && cz == 0,
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

    // A 5x5 pool of sources on a stone floor at y=64, plus one explicitly
    // flowing cell (falloff 4) at the east rim that opens onto air.
    let mut chunk = Chunk::new(0, 0);
    for z in 6..=10 {
        for x in 6..=10 {
            chunk.set_block(x, 64, z, Block::Stone);
            chunk.set_water(x, 65, z, Block::Water, 0); // source
        }
    }
    chunk.set_block(11, 64, 8, Block::Stone);
    chunk.set_water(11, 65, 8, Block::Water, 4); // flowing, opens east onto air

    let air = |_: i32, _: i32, _: i32| 0u8;
    let water0 = |_: i32, _: i32, _: i32| 0u8;
    let biome0 = |_: i32, _: i32| 0u8;
    let light = |_: i32, _: i32, _: i32| SKY_FULL;
    let mesh = build_mesh_lods_with_loaded_neighbors(
        &chunk,
        air,
        water0,
        biome0,
        light,
        |_, _, _| 0u8,
        |_, _| true,
    );

    // Decode the upward-facing tile for each top vertex (those raised above the
    // cell floor). Collect tile ids and the lowest/highest surface heights.
    let mut saw_still = false;
    let mut saw_flow = false;
    let mut min_top = f32::INFINITY;
    let mut max_top: f32 = 0.0;
    for v in &mesh.transparent {
        let tile = v.packed & 0xFFu32;
        // Top vertices sit above the cell base (y=65); skip the side/bottom ones.
        if v.pos[1] > 65.05 {
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
        max_top <= 65.9,
        "water tops are recessed below the full block (got {max_top})"
    );
    assert!(
        min_top < 65.6,
        "the flowing cell should slope notably lower than a source (got {min_top})"
    );
}

/// A submerged (capped) water cell must render its side face toward a shorter
/// open-surface neighbour — the exposed vertical step — so a 1-deep flow stepping
/// down doesn't show the floor through the height gap.
#[test]
fn submerged_water_renders_exposed_step_toward_a_shorter_neighbour() {
    let mut chunk = Chunk::new(0, 0);
    chunk.set_block(8, 63, 8, Block::Stone);
    chunk.set_block(9, 63, 8, Block::Stone);
    // 2-deep column at x=8 -> the y=64 cell is capped (water above) and full.
    chunk.set_water(8, 64, 8, Block::Water, 0);
    chunk.set_water(8, 65, 8, Block::Water, 0);
    // Shorter open-surface flowing cell next door (air above it).
    chunk.set_water(9, 64, 8, Block::Water, 3);

    let mesh = build_mesh_lods_with_loaded_neighbors(
        &chunk,
        |_, _, _| 0u8,
        |_, _, _| 0u8,
        |_, _| 0u8,
        |_, _, _| SKY_FULL,
        |_, _, _| 0u8,
        |_, _| true,
    );

    // The capped cell's east face lives on the x=9 plane. It is rendered (not
    // culled water<->water) as a BAND: trimmed at the bottom to the neighbour's
    // recessed surface (~0.79 here) and full at the top, so the submerged part
    // (water behind water) isn't drawn. The trimmed bottom edge is the only water
    // vertex on that plane strictly inside (64, 65); a culled or full-height face
    // would have none there.
    let band_bottom = mesh.transparent.iter().any(|v| {
        (v.pos[0] - 9.0).abs() < 1e-3 && shade_idx(v) == 2 && v.pos[1] > 64.05 && v.pos[1] < 64.95
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

    let mut chunk = Chunk::new(0, 0);
    chunk.set_block(8, 63, 8, Block::Stone);
    chunk.set_block(9, 63, 8, Block::Stone);
    chunk.set_water(8, 64, 8, Block::Water, FALLING_META);
    chunk.set_water(9, 64, 8, Block::Water, 3);

    let mesh = build_mesh_lods_with_loaded_neighbors(
        &chunk,
        |_, _, _| 0u8,
        |_, _, _| 0u8,
        |_, _| 0u8,
        |_, _, _| SKY_FULL,
        |_, _, _| 0u8,
        |_, _| true,
    );

    let step = mesh
        .transparent
        .iter()
        .filter(|v| (v.pos[0] - 9.0).abs() < 1e-3 && shade_idx(v) == 2)
        .collect::<Vec<_>>();

    assert!(
        step.iter().any(|v| (v.pos[1] - 65.0).abs() < 1e-3),
        "falling water step must reach the full cell top"
    );
    assert!(
        step.iter().any(|v| v.pos[1] > 64.05 && v.pos[1] < 64.95),
        "falling water step must be trimmed to the neighbour's lower surface"
    );
}

fn shade_idx(v: &Vertex) -> u32 {
    (v.packed >> 10) & 0x3
}

fn light6(v: &Vertex) -> u32 {
    (v.packed >> 23) & 0x3F
}

fn uv_mode(v: &Vertex) -> u32 {
    (v.packed >> super::vertex::UV_MODE_SHIFT) & 0x7
}

/// Sampler over a computed skylight band, for the skylight unit tests.
struct TestSky {
    band: Box<[u8]>,
    ylo: i32,
    yhi: i32,
}

impl TestSky {
    fn at(&self, x: i32, y: i32, z: i32) -> u8 {
        if y > self.yhi {
            return SKY_FULL;
        }
        if y < self.ylo {
            return 0;
        }
        let ay = y - self.ylo;
        self.band[((ay * CHUNK_SZ as i32 + z) * CHUNK_SX as i32 + x) as usize]
    }
}

fn solo_skylight(c: &Chunk) -> TestSky {
    let (band, ylo, yhi) = compute_chunk_skylight(c);
    TestSky { band, ylo, yhi }
}

/// Mesh a standalone chunk: bake its self-contained skylight, then build the
/// mesh sampling that cached light (out-of-chunk reads as open sky).
fn mesh_solo(c: &mut Chunk) -> ChunkMesh {
    mesh_solo_with_options(c, MeshOptions::DETAILED)
}

fn mesh_solo_with_options(c: &mut Chunk, options: MeshOptions) -> ChunkMesh {
    let (band, ylo, yhi) = compute_chunk_skylight(c);
    c.set_skylight(band, ylo, yhi);
    build_mesh_with_options(
        &*c,
        |_, _, _| 0u8,
        |_, _| 4u8,
        |wx, wy, wz| {
            if wx < 0
                || wx >= CHUNK_SX as i32
                || wz < 0
                || wz >= CHUNK_SZ as i32
                || wy < 0
                || wy >= CHUNK_SY as i32
            {
                SKY_FULL
            } else {
                c.skylight_at(wx as usize, wy, wz as usize)
            }
        },
        options,
    )
}

/// Open columns are full sky (15 = 30 on the x2 scale), and nothing exceeds it.
#[test]
fn skylight_open_column_is_full() {
    let mut c = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            c.set_block(x, 0, z, Block::Stone);
        }
    }
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
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            for y in 0..=80 {
                c.set_block(x, y, z, Block::Stone);
            }
        }
    }
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
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            for y in 0..=6 {
                c.set_block(x, y, z, Block::Stone);
            }
        }
    }
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
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                for y in 0..=6 {
                    c.set_block(x, y, z, Block::Stone);
                }
            }
        }
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

/// Build an opaque-walled vertical shaft of `fill` from y=1..=8 over a floor,
/// so the only light path is straight down through `fill`.
fn walled_shaft(fill: Block) -> Chunk {
    let mut c = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            c.set_block(x, 0, z, Block::Stone);
        }
    }
    for y in 1..=8 {
        c.set_block(8, y, 8, fill);
        c.set_block(7, y, 8, Block::Stone);
        c.set_block(9, y, 8, Block::Stone);
        c.set_block(8, y, 7, Block::Stone);
        c.set_block(8, y, 9, Block::Stone);
    }
    c
}

/// Water attenuates a FULL light level per layer (2 on the x2 scale = 1 real),
/// the same rate as air -- so light drops off quickly underwater.
#[test]
fn skylight_water_attenuates_one_level_per_layer() {
    let sky = solo_skylight(&walled_shaft(Block::Water));
    assert_eq!(sky.at(8, 9, 8), SKY_FULL, "air above the water is full sky");
    assert_eq!(sky.at(8, 8, 8), SKY_FULL - 2);
    assert_eq!(sky.at(8, 7, 8), SKY_FULL - 4);
    assert_eq!(sky.at(8, 6, 8), SKY_FULL - 6);
}

/// Leaves still attenuate at HALF rate (1 on the x2 scale = 0.5 real), so
/// light reaches deeper into a canopy than into water.
#[test]
fn skylight_leaves_attenuate_half() {
    let sky = solo_skylight(&walled_shaft(Block::OakLeaves));
    assert_eq!(
        sky.at(8, 9, 8),
        SKY_FULL,
        "air above the leaves is full sky"
    );
    assert_eq!(sky.at(8, 8, 8), SKY_FULL - 1);
    assert_eq!(sky.at(8, 7, 8), SKY_FULL - 2);
    assert_eq!(sky.at(8, 6, 8), SKY_FULL - 3);
}

/// Leaves occlude AO onto/within themselves: a solid leaf cluster floating in
/// air must produce darkened (ao < 3) leaf faces -- interior faces are buried
/// by surrounding leaves. (Before, leaves never occluded, so AO stayed 3.)
#[test]
fn leaves_self_occlude() {
    assert!(Block::OakLeaves.occludes_ao());
    assert!(!Block::Water.occludes_ao());
    assert!(!Block::Air.occludes_ao());

    let mut c = Chunk::new(0, 0);
    for y in 5..=7 {
        for z in 7..=9 {
            for x in 7..=9 {
                c.set_block(x, y, z, Block::OakLeaves);
            }
        }
    }
    let mesh = mesh_solo(&mut c);
    assert!(
        !mesh.opaque.is_empty(),
        "leaf cluster should mesh (cutout opaque pass)"
    );
    let min_ao = mesh
        .opaque
        .iter()
        .map(|v| (v.packed >> 21) & 0x3)
        .min()
        .unwrap();
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

/// Stone floor (y=0..=4) over the whole chunk, so test columns are not open
/// below -- keeps the volumetric descent the only thing under study.
fn floored_chunk() -> Chunk {
    let mut c = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            for y in 0..=4 {
                c.set_block(x, y, z, Block::Stone);
            }
        }
    }
    c
}

/// Volumetric depth darkening: the air BELOW a leaf canopy keeps losing 0.5 a
/// level (1 on the x2 scale) per block of descent, not just at the leaf -- so it
/// gets darker the deeper you go under cover (and digging down stays dark, see
/// `skylight_digging_down_under_cover_keeps_darkening`).
#[test]
fn skylight_air_below_canopy_darkens_with_depth() {
    let mut c = floored_chunk();
    // A leaf roof at y=10 over the whole chunk; open air pocket y=5..=9 below.
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            c.set_block(x, 10, z, Block::OakLeaves);
        }
    }
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
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            for y in 6..=10 {
                c.set_block(x, y, z, Block::Water);
            }
        }
    }
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
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            c.set_block(x, 10, z, Block::OakLeaves);
        }
    }
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
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            for y in 6..=10 {
                c.set_block(x, y, z, Block::Water);
            }
        }
    }
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

fn roof_with_open_shaft(roof: Block) -> Chunk {
    let mut c = floored_chunk();
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            c.set_block(x, 10, z, roof);
        }
    }
    c.set_block(8, 10, 8, Block::Air);
    c
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
    let mut c = Chunk::new(0, 0);
    // The step block.
    c.set_block(8, 64, 8, Block::Stone);
    // The 2-tall pillar one cell over in +X; its upper cube (9,65,8) is the
    // single edge-occluder of the step block's top (+Y) face.
    c.set_block(9, 64, 8, Block::Stone);
    c.set_block(9, 65, 8, Block::Stone);
    let mesh = mesh_solo(&mut c);

    // The step block's top face is the only +Y (PosY -> shade idx 0) quad whose
    // four corners lie at y == 65 over the step cell x in [8,9], z in [8,9].
    let ao_at = |wx: f32, wz: f32| -> u32 {
        let v = mesh
            .opaque
            .iter()
            .find(|v| {
                (v.packed >> 10) & 0x3 == 0 // PosY
                    && (v.pos[1] - 65.0).abs() < 1e-3
                    && (v.pos[0] - wx).abs() < 1e-3
                    && (v.pos[2] - wz).abs() < 1e-3
            })
            .unwrap_or_else(|| panic!("no top-face vertex at ({wx}, 65, {wz})"));
        (v.packed >> 21) & 0x3
    };

    // The two corners on the shared +X edge (x == 9), adjacent to the pillar:
    // one solid edge neighbour each -> ao == 2.
    assert_eq!(ao_at(9.0, 8.0), 2, "concave +X corner is edge-occluded");
    assert_eq!(ao_at(9.0, 9.0), 2, "concave +X corner is edge-occluded");
    // The two corners on the open -X edge (x == 8): no occluder -> ao == 3.
    assert_eq!(ao_at(8.0, 8.0), 3, "open -X corner is fully lit");
    assert_eq!(ao_at(8.0, 9.0), 3, "open -X corner is fully lit");
}

#[test]
fn stair_bottom_face_uses_the_dark_cell_below_not_smooth_sky_leak() {
    let pos = crate::chunk::SectionPos::new(0, 0, 0);
    let mut section = crate::section::Section::new(0, 0, 0);
    section.set_block(8, 8, 8, Block::OakStairs);
    section.set_stair_facing(8, 8, 8, crate::furnace::Facing::East);

    let mesh = super::build_section_mesh(
        &section,
        pos,
        |wx, wy, wz| {
            if (wx, wy, wz) == (8, 8, 8) {
                Block::OakStairs.id()
            } else {
                Block::Air.id()
            }
        },
        |_, _, _| crate::furnace::Facing::North,
        |_, _, _| 0,
        |_, _| 0,
        |wx, wy, wz| {
            if wy == 7 && (wx, wz) != (8, 8) {
                SKY_FULL
            } else {
                0
            }
        },
        |_, _, _| 0,
        |_, _, _| true,
    );

    let bottom = mesh
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 3 && (v.pos[1] - 8.0).abs() < 1.0e-3)
        .collect::<Vec<_>>();
    assert!(!bottom.is_empty(), "stair should emit bottom-face vertices");
    assert!(
        bottom.iter().all(|v| light6(v) == 0),
        "a stair's solid bottom must not show skylight from adjacent below cells"
    );
}

#[test]
fn stair_faces_use_cell_local_uv_modes() {
    use super::vertex::{
        UV_MODE_NONE, UV_MODE_STAIR_NEG_X, UV_MODE_STAIR_NEG_Z, UV_MODE_STAIR_POS_X,
        UV_MODE_STAIR_POS_Z, UV_MODE_STAIR_TOP,
    };
    use crate::furnace::Facing;

    let pos = crate::chunk::SectionPos::new(0, 0, 0);
    let mut section = crate::section::Section::new(0, 0, 0);
    section.set_block(8, 8, 8, Block::RedwoodStairs);
    section.set_stair_facing(8, 8, 8, Facing::South);

    let mesh = super::build_section_mesh(
        &section,
        pos,
        |wx, wy, wz| {
            if (wx, wy, wz) == (8, 8, 8) {
                Block::RedwoodStairs.id()
            } else {
                Block::Air.id()
            }
        },
        |_, _, _| crate::furnace::Facing::North,
        |_, _, _| 0,
        |_, _| 0,
        |_, _, _| SKY_FULL,
        |_, _, _| 0,
        |_, _, _| true,
    );

    let expect_mode = |x: f32, y: f32, z: f32, mode: u32| {
        let v = mesh
            .opaque
            .iter()
            .find(|v| {
                (v.pos[0] - x).abs() < 1.0e-3
                    && (v.pos[1] - y).abs() < 1.0e-3
                    && (v.pos[2] - z).abs() < 1.0e-3
                    && uv_mode(v) == mode
            })
            .unwrap_or_else(|| panic!("no stair vertex at ({x}, {y}, {z}) with UV mode {mode}"));
        assert_eq!(uv_mode(v), mode);
    };

    expect_mode(9.0, 8.0, 8.0, UV_MODE_STAIR_POS_X);
    expect_mode(8.0, 8.0, 8.0, UV_MODE_STAIR_NEG_X);
    expect_mode(8.0, 8.0, 9.0, UV_MODE_STAIR_POS_Z);
    expect_mode(8.0, 8.0, 8.0, UV_MODE_STAIR_NEG_Z);
    expect_mode(8.0, 8.5, 9.0, UV_MODE_STAIR_TOP);
    expect_mode(8.0, 9.0, 8.0, UV_MODE_STAIR_TOP);

    assert!(
        mesh.opaque
            .iter()
            .filter(|v| (v.pos[1] - 8.0).abs() < 1.0e-3)
            .any(|v| uv_mode(v) == UV_MODE_NONE),
        "stair bottom faces should keep normal full-tile UVs"
    );
}

#[test]
fn stair_mesh_uses_resolved_outside_corner_shape() {
    use crate::furnace::Facing;

    let pos = crate::chunk::SectionPos::new(0, 0, 0);
    let mut section = crate::section::Section::new(0, 0, 0);
    section.set_block(8, 8, 8, Block::OakStairs);
    section.set_stair_facing(8, 8, 8, Facing::East);
    section.set_block(7, 8, 8, Block::OakStairs);
    section.set_stair_facing(7, 8, 8, Facing::South);

    let mesh = super::build_section_mesh(
        &section,
        pos,
        |wx, wy, wz| match (wx, wy, wz) {
            (8, 8, 8) | (7, 8, 8) => Block::OakStairs.id(),
            _ => Block::Air.id(),
        },
        |wx, wy, wz| match (wx, wy, wz) {
            (8, 8, 8) => Facing::East,
            (7, 8, 8) => Facing::South,
            _ => Facing::North,
        },
        |_, _, _| 0,
        |_, _| 0,
        |_, _, _| SKY_FULL,
        |_, _, _| 0,
        |_, _, _| true,
    );

    let target_high_top = mesh
        .opaque
        .iter()
        .filter(|v| {
            (v.pos[1] - 9.0).abs() < 1.0e-3
                && v.pos[0] >= 8.0 - 1.0e-3
                && v.pos[0] < 9.0 - 1.0e-3
                && v.pos[2] >= 8.0 - 1.0e-3
                && v.pos[2] < 9.0 - 1.0e-3
        })
        .collect::<Vec<_>>();

    assert!(
        !target_high_top.is_empty(),
        "target stair should still have one high quadrant"
    );
    assert!(
        target_high_top
            .iter()
            .all(|v| v.pos[0] <= 8.5 + 1.0e-3 && v.pos[2] <= 8.5 + 1.0e-3),
        "the high-side perpendicular neighbour must render an outside corner"
    );
}

/// Parallel mesh building (World::tick_mesh_budget on native) must produce
/// byte-identical meshes to a serial build: `build_mesh` is a pure function of
/// (chunk, neighbour reads) with no shared mutable state, so rayon only reorders
/// independent work.
mod parallel_parity_tests {
    use super::*;
    use crate::chunk::{Chunk, CHUNK_SY, SKY_FULL};
    use crate::worldgen::generate_chunk;
    use rayon::prelude::*;
    use std::collections::HashMap;

    /// The skylight bake may run on worker/rayon threads in tools and tests, so
    /// it must be deterministic: same blocks -> byte-identical band, regardless
    /// of thread or repetition (guards the per-thread `SKY_SCRATCH` being fully
    /// reset each call and the flood being order-independent).
    #[test]
    fn skylight_bake_is_deterministic_serial_vs_parallel() {
        let seed = 0x1234_5678u32;
        let coords: Vec<(i32, i32)> = (-2..=2)
            .flat_map(|cz| (-2..=2).map(move |cx| (cx, cz)))
            .collect();
        let chunks: Vec<Chunk> = coords
            .iter()
            .map(|&(cx, cz)| generate_chunk(seed, cx, cz))
            .collect();

        let serial: Vec<(Box<[u8]>, i32, i32)> =
            chunks.iter().map(compute_chunk_skylight).collect();

        // Same chunk baked twice back-to-back on one thread -> identical (scratch reset).
        for (c, s) in chunks.iter().zip(&serial) {
            let again = compute_chunk_skylight(c);
            assert_eq!(&again.0[..], &s.0[..]);
            assert_eq!((again.1, again.2), (s.1, s.2));
        }

        // Parallel bake (mirrors World::poll) -> byte-identical to serial.
        let parallel: Vec<(Box<[u8]>, i32, i32)> =
            chunks.par_iter().map(compute_chunk_skylight).collect();
        for (p, s) in parallel.iter().zip(&serial) {
            assert_eq!(
                &p.0[..],
                &s.0[..],
                "parallel skylight bake differs from serial"
            );
            assert_eq!((p.1, p.2), (s.1, s.2));
        }
    }

    #[test]
    fn parallel_meshing_is_byte_identical_to_serial() {
        let seed = 0x1234_5678u32;
        let coords: Vec<(i32, i32)> = (-2..=2)
            .flat_map(|cz| (-2..=2).map(move |cx| (cx, cz)))
            .collect();
        let chunks: HashMap<(i32, i32), Chunk> = coords
            .iter()
            .map(|&(cx, cz)| {
                let mut c = generate_chunk(seed, cx, cz);
                let (band, ylo, yhi) = compute_chunk_skylight(&c);
                c.set_skylight(band, ylo, yhi);
                ((cx, cz), c)
            })
            .collect();

        let mesh_one = |&(cx, cz): &(i32, i32)| -> ChunkMesh {
            let c = &chunks[&(cx, cz)];
            let nb = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 || wy >= CHUNK_SY as i32 {
                    return 0;
                }
                match chunks.get(&(wx >> 4, wz >> 4)) {
                    Some(c) => c.block_raw((wx & 15) as usize, wy as usize, (wz & 15) as usize),
                    None => 0,
                }
            };
            let nb_biome = |wx: i32, wz: i32| -> u8 {
                match chunks.get(&(wx >> 4, wz >> 4)) {
                    Some(c) => c.biome_at((wx & 15) as usize, (wz & 15) as usize),
                    None => 0,
                }
            };
            let nb_light = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 {
                    return 0;
                }
                if wy >= CHUNK_SY as i32 {
                    return SKY_FULL;
                }
                match chunks.get(&(wx >> 4, wz >> 4)) {
                    Some(c) => c.skylight_at((wx & 15) as usize, wy, (wz & 15) as usize),
                    None => SKY_FULL,
                }
            };
            build_mesh(c, nb, nb_biome, nb_light)
        };

        let serial: Vec<ChunkMesh> = coords.iter().map(mesh_one).collect();
        let parallel: Vec<ChunkMesh> = coords.par_iter().map(mesh_one).collect();

        for (s, p) in serial.iter().zip(&parallel) {
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.opaque),
                bytemuck::cast_slice::<Vertex, u8>(&p.opaque),
            );
            assert_eq!(s.opaque_idx, p.opaque_idx);
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.transparent),
                bytemuck::cast_slice::<Vertex, u8>(&p.transparent),
            );
            assert_eq!(s.transparent_idx, p.transparent_idx);
        }
    }
}

/// A placed furnace shows its front on exactly the face it was placed facing and
/// `furnace_side` on the other three horizontal faces (top + bottom use the top
/// tile). Pins the directional fix for "front rendered on all four sides".
#[test]
fn furnace_shows_front_on_facing_face_and_side_on_the_others() {
    use crate::atlas::Tile;
    use crate::furnace::{Facing, Furnace};
    let air = |_: i32, _: i32, _: i32| 0u8;
    let biome0 = |_: i32, _: i32| 0u8;
    let light = |_: i32, _: i32, _: i32| SKY_FULL;

    let mut chunk = Chunk::new(0, 0);
    chunk.set_block(8, 64, 8, Block::Furnace);
    chunk.insert_furnace(
        8,
        64,
        8,
        Furnace {
            facing: Facing::East,
            ..Default::default()
        },
    );

    let count = |mesh: &ChunkMesh, tile: Tile| {
        mesh.opaque
            .iter()
            .filter(|v| v.packed & 0xFF == tile.index() as u32)
            .count()
    };

    // Unlit: 6 faces × 4 verts — 1 front, 3 sides, 2 top/bottom.
    let m = build_mesh(&chunk, air, biome0, light);
    assert_eq!(
        count(&m, Tile::named("furnace_front")),
        4,
        "front on exactly the facing face"
    );
    assert_eq!(
        count(&m, Tile::named("furnace_side")),
        12,
        "side on the other three faces"
    );
    assert_eq!(count(&m, Tile::named("furnace_top")), 8, "top + bottom");
    assert_eq!(
        count(&m, Tile::named("furnace_front_on")),
        0,
        "no lit front while unlit"
    );

    // Lit: the facing face swaps to the glowing front; the sides do not glow.
    chunk.furnace_at_mut(8, 64, 8).unwrap().burn_remaining = 100;
    chunk.dirty = true;
    let lit = build_mesh(&chunk, air, biome0, light);
    assert_eq!(
        count(&lit, Tile::named("furnace_front_on")),
        4,
        "lit front on the facing face only"
    );
    assert_eq!(count(&lit, Tile::named("furnace_front")), 0);
    assert_eq!(count(&lit, Tile::named("furnace_side")), 12, "sides never glow");
}

/// Greedy meshing collapses a flat, uniformly-lit region of identical opaque faces into a
/// single tiled quad, and encodes the merge extent so the shader tiles the tile W×H. A 16×16
/// stone floor's top faces (all AO=3, full sky, same tile+tint) must merge to ONE quad whose
/// packed W,H = 16, and the whole section's opaque geometry must collapse far below the
/// per-cell face count. Pins the merge condition + the (W-1,H-1) packing the shader decodes.
#[test]
fn greedy_merges_flat_floor_into_tiled_quads() {
    use crate::chunk::SectionPos;
    use crate::furnace::Facing;
    use crate::section::Section;

    let pos = SectionPos::new(0, 0, 0);
    let mut section = Section::new(0, 0, 0);
    for lz in 0..CHUNK_SZ {
        for lx in 0..CHUNK_SX {
            section.set_block(lx, 0, lz, Block::Stone);
        }
    }
    section.recompute_opaque_count();

    let mesh = super::build_section_mesh(
        &section,
        pos,
        |wx, wy, wz| {
            if wy == 0 && (0..16).contains(&wx) && (0..16).contains(&wz) {
                Block::Stone.id()
            } else {
                Block::Air.id()
            }
        },
        |_, _, _| Facing::North,
        |_, _, _| 0,
        |_, _| 0,
        |_, _, _| SKY_FULL,
        |_, _, _| 0,
        |_, _, _| true,
    );

    // The 16×16 top (+Y, shade idx 0) at y=1 collapses to a single 4-vertex quad.
    let top: Vec<&Vertex> = mesh
        .opaque
        .iter()
        .filter(|v| shade_idx(v) == 0 && (v.pos[1] - 1.0).abs() < 1e-3)
        .collect();
    assert_eq!(top.len(), 4, "flat 16×16 top should merge into one quad");
    let w = ((top[0].packed >> 12) & 0xF) + 1;
    let h = ((top[0].packed >> 16) & 0xF) + 1;
    assert_eq!(
        (w, h),
        (16, 16),
        "merged top quad must tile its layer 16×16"
    );
    // The quad covers at least the full section footprint; greedy quads may overlap their
    // tangent edges slightly so long T-junctions do not show background cracks.
    let (min_x, max_x) = (
        top.iter().map(|v| v.pos[0]).fold(f32::INFINITY, f32::min),
        top.iter()
            .map(|v| v.pos[0])
            .fold(f32::NEG_INFINITY, f32::max),
    );
    assert!(min_x <= 0.0 && max_x >= 16.0);

    // Per cell this floor would emit 256 top + 256 bottom + 4×16 side faces = 576 quads
    // (2304 verts); greedy collapses it to a handful.
    assert!(
        mesh.opaque.len() < 64,
        "greedy should collapse the flat floor, got {} verts",
        mesh.opaque.len()
    );
}

#[test]
fn pad_local_section_mesher_matches_closure_mesher() {
    use crate::chunk::{SectionPos, SECTION_SIZE, SKY_FULL};
    use crate::furnace::{Facing, Furnace};
    use crate::section::Section;

    const PAD: usize = SECTION_SIZE + 2;
    const PAD_VOL: usize = PAD * PAD * PAD;
    const BIOME_PAD_RADIUS: i32 = 2;
    const BIOME_PAD: usize = SECTION_SIZE + (BIOME_PAD_RADIUS as usize * 2);
    let pidx = |x: usize, y: usize, z: usize| (y * PAD + z) * PAD + x;
    let bidx = |x: usize, z: usize| z * BIOME_PAD + x;

    let pos = SectionPos::new(0, 0, 0);
    let mut section = Section::new(0, 0, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            section.set_block(x, 0, z, Block::Stone);
        }
    }
    section.set_block(2, 1, 2, Block::Grass);
    section.set_block(3, 1, 2, Block::OakLeaves);
    section.set_block(4, 1, 2, Block::ShortGrass);
    section.set_water(5, 1, 2, Block::Water, 4);
    section.set_block(6, 1, 2, Block::Furnace);
    section.insert_furnace(
        6,
        1,
        2,
        Furnace {
            facing: Facing::East,
            burn_remaining: 10,
            ..Default::default()
        },
    );
    section.set_block(7, 1, 2, Block::Cactus);
    section.set_block(8, 1, 2, Block::OakStairs);
    section.set_stair_facing(8, 1, 2, Facing::South);

    let block_at = |wx: i32, wy: i32, wz: i32| -> u8 {
        if (0..SECTION_SIZE as i32).contains(&wx)
            && (0..SECTION_SIZE as i32).contains(&wy)
            && (0..SECTION_SIZE as i32).contains(&wz)
        {
            section.block_raw(wx as usize, wy as usize, wz as usize)
        } else if wy == 0
            && (-1..=SECTION_SIZE as i32).contains(&wx)
            && (-1..=SECTION_SIZE as i32).contains(&wz)
        {
            Block::Stone.id()
        } else {
            Block::Air.id()
        }
    };
    let water_at = |wx: i32, wy: i32, wz: i32| -> u8 {
        if (0..SECTION_SIZE as i32).contains(&wx)
            && (0..SECTION_SIZE as i32).contains(&wy)
            && (0..SECTION_SIZE as i32).contains(&wz)
        {
            section.water_meta(wx as usize, wy as usize, wz as usize)
        } else {
            0
        }
    };
    let stair_at = |wx: i32, wy: i32, wz: i32| -> Facing {
        if (0..SECTION_SIZE as i32).contains(&wx)
            && (0..SECTION_SIZE as i32).contains(&wy)
            && (0..SECTION_SIZE as i32).contains(&wz)
        {
            section.stair_facing(wx as usize, wy as usize, wz as usize)
        } else {
            Facing::North
        }
    };
    let sky_at = |wx: i32, wy: i32, wz: i32| -> u8 {
        if wy < 0 {
            0
        } else if wy >= SECTION_SIZE as i32 {
            SKY_FULL
        } else {
            (18 + (wx * 3 + wy * 5 + wz * 7).rem_euclid(13)) as u8
        }
    };
    let blocklight_at =
        |wx: i32, wy: i32, wz: i32| -> u8 { ((wx + wy * 2 + wz * 3).rem_euclid(5) * 2) as u8 };
    let biome_at = |_: i32, _: i32| -> u8 { 0 };
    let loaded_at = |_: i32, _: i32, _: i32| -> bool { true };

    let serial = build_section_mesh(
        &section,
        pos,
        block_at,
        stair_at,
        water_at,
        biome_at,
        sky_at,
        blocklight_at,
        loaded_at,
    );

    let mut blocks = vec![0u8; PAD_VOL];
    let mut water = vec![0u8; PAD_VOL];
    let mut skylight = vec![SKY_FULL; PAD_VOL];
    let mut blocklight = vec![0u8; PAD_VOL];
    let mut stair_facings = vec![Facing::North.to_u8(); PAD_VOL];
    let loaded = vec![true; PAD_VOL];
    for py in 0..PAD {
        for pz in 0..PAD {
            for px in 0..PAD {
                let (wx, wy, wz) = (px as i32 - 1, py as i32 - 1, pz as i32 - 1);
                let i = pidx(px, py, pz);
                blocks[i] = block_at(wx, wy, wz);
                water[i] = water_at(wx, wy, wz);
                skylight[i] = sky_at(wx, wy, wz);
                blocklight[i] = blocklight_at(wx, wy, wz);
                stair_facings[i] = stair_at(wx, wy, wz).to_u8();
            }
        }
    }
    let mut biome = vec![0u8; BIOME_PAD * BIOME_PAD];
    for pz in 0..BIOME_PAD {
        for px in 0..BIOME_PAD {
            biome[bidx(px, pz)] =
                biome_at(px as i32 - BIOME_PAD_RADIUS, pz as i32 - BIOME_PAD_RADIUS);
        }
    }

    let pad = build_section_mesh_from_pad(
        &section,
        pos,
        SectionMeshPad {
            blocks: &blocks,
            water: &water,
            skylight: &skylight,
            blocklight: &blocklight,
            stair_facings: &stair_facings,
            loaded: &loaded,
            biome: &biome,
        },
    );

    assert_eq!(
        bytemuck::cast_slice::<Vertex, u8>(&serial.opaque),
        bytemuck::cast_slice::<Vertex, u8>(&pad.opaque)
    );
    assert_eq!(serial.opaque_idx, pad.opaque_idx);
    assert_eq!(
        bytemuck::cast_slice::<Vertex, u8>(&serial.transparent),
        bytemuck::cast_slice::<Vertex, u8>(&pad.transparent)
    );
    assert_eq!(serial.transparent_idx, pad.transparent_idx);
    assert_eq!(
        bytemuck::cast_slice::<Vertex, u8>(&serial.far_opaque),
        bytemuck::cast_slice::<Vertex, u8>(&pad.far_opaque)
    );
    assert_eq!(serial.far_opaque_idx, pad.far_opaque_idx);
    assert_eq!(
        bytemuck::cast_slice::<ModelVertex, u8>(&serial.model),
        bytemuck::cast_slice::<ModelVertex, u8>(&pad.model)
    );
    assert_eq!(serial.model_idx, pad.model_idx);
    assert_eq!(serial.mesh_dirty, pad.mesh_dirty);
}
