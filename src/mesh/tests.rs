use super::face::{should_flip, vertex_ao, FACES};
use super::*;
use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};
use crate::worldgen::generate_chunk;

/// The packed shade index must decode (via SHADES) to the same float the old
/// per-vertex `Face::shade()` produced -- and SHADES must match the literal
/// table in block.wgsl. Guards the index<->value mapping against drift.
#[test]
fn shade_table_matches_face_shade() {
    for f in FACES {
        assert_eq!(
            SHADES[f.shade_idx() as usize],
            f.shade(),
            "shade idx/value drift for {f:?}"
        );
    }
    // Mirror of block.wgsl's `array<f32,4>(...)`.
    assert_eq!(SHADES, [1.00, 0.85, 0.75, 0.55]);
}

/// Leaves must render in the OPAQUE pass, not the alpha-blended one. Proof: a
/// chunk that has leaves but NO water must produce an empty transparent buffer
/// (only water feeds it now) and a non-empty opaque buffer.
#[test]
fn leaves_go_to_opaque_pass() {
    let seed = 0x1234_5678u32;
    for cz in 0..16 {
        for cx in 0..16 {
            let mut c = generate_chunk(seed, cx, cz);
            let (mut leaf, mut water) = (false, false);
            for y in 0..CHUNK_SY {
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        match Block::from_id(c.block_raw(x, y, z)) {
                            Block::OakLeaves => leaf = true,
                            Block::Water => water = true,
                            _ => {}
                        }
                    }
                }
            }
            if leaf && !water {
                let mesh = mesh_solo(&mut c);
                assert!(
                    mesh.transparent_idx.is_empty(),
                    "leaves+no-water chunk should have an empty transparent buffer"
                );
                assert!(
                    !mesh.opaque_idx.is_empty(),
                    "leaves should fill the opaque buffer"
                );
                return;
            }
        }
    }
    panic!("no leaf-bearing, water-free chunk found to test");
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
    let (band, ylo, yhi) = compute_chunk_skylight(c);
    c.set_skylight(band, ylo, yhi);
    build_mesh(
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

/// AO must actually be computed and vary: real terrain has both fully-lit
/// (ao=3) corners and occluded (ao<3) ones. Scans a small chunk grid so the
/// assertion can't hinge on one unlucky flat chunk.
#[test]
fn ao_varies_across_generated_terrain() {
    let seed = 0x1234_5678u32;
    let (mut saw_open, mut saw_occluded) = (false, false);
    'outer: for cz in 0..3 {
        for cx in 0..3 {
            let mut c = generate_chunk(seed, cx, cz);
            let mesh = mesh_solo(&mut c);
            for v in &mesh.opaque {
                match (v.packed >> 21) & 0x3 {
                    3 => saw_open = true,
                    _ => saw_occluded = true,
                }
                if saw_open && saw_occluded {
                    break 'outer;
                }
            }
        }
    }
    assert!(saw_open, "expected some fully-lit (ao=3) vertices");
    assert!(
        saw_occluded,
        "expected some occluded (ao<3) vertices in real terrain"
    );
}

/// Parallel mesh building (World::tick_mesh_budget on native) must produce
/// byte-identical meshes to a serial build: `build_mesh` is a pure function of
/// (chunk, neighbour reads) with no shared mutable state, so rayon only reorders
/// independent work. This locks that invariant down objectively (perfbench
/// meshes serially and never exercises the rayon path).
#[cfg(not(target_arch = "wasm32"))]
mod parallel_parity_tests {
    use super::*;
    use crate::chunk::{Chunk, CHUNK_SY, SKY_FULL};
    use crate::worldgen::generate_chunk;
    use rayon::prelude::*;
    use std::collections::HashMap;

    /// The skylight bake runs under rayon (`World::poll`), so it must be
    /// deterministic: same blocks -> byte-identical band, regardless of thread or
    /// repetition (guards the per-thread `SKY_SCRATCH` being fully reset each call
    /// and the flood being order-independent).
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
