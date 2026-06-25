//! Throwaway perf harness: times worldgen + meshing over a chunk grid so we can
//! measure CPU-side optimizations with real before/after numbers.
//!
//! Run: `cargo run --release --bin perfbench [grid_radius] [iters]`

use std::collections::HashMap;
use std::time::Instant;

use llamacraft::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};
use llamacraft::mesh::{build_mesh, compute_chunk_skylight_with_neighbors};
use llamacraft::worldgen::generate_chunk;

fn main() {
    let mut args = std::env::args().skip(1);
    let r: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
    let iters: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let seed = 0x1234_5678u32;

    let coords: Vec<(i32, i32)> = (-r..=r)
        .flat_map(|cz| (-r..=r).map(move |cx| (cx, cz)))
        .collect();
    let n = coords.len();
    println!(
        "grid: {}x{} = {} chunks, seed={:08x}, iters={}",
        2 * r + 1,
        2 * r + 1,
        n,
        seed,
        iters
    );

    // ---- worldgen ----
    let mut chunks: HashMap<(i32, i32), Chunk> = HashMap::new();
    let mut gen_ns = u128::MAX;
    for it in 0..iters {
        let t = Instant::now();
        let mut local: HashMap<(i32, i32), Chunk> = HashMap::with_capacity(n);
        for &(cx, cz) in &coords {
            local.insert((cx, cz), generate_chunk(seed, cx, cz));
        }
        let e = t.elapsed().as_nanos();
        gen_ns = gen_ns.min(e);
        if it + 1 == iters {
            chunks = local;
        }
    }
    println!(
        "worldgen : {:>8.2} ms total | {:>7.3} ms/chunk  (best of {})",
        gen_ns as f64 / 1e6,
        gen_ns as f64 / 1e6 / n as f64,
        iters
    );

    // ---- worldgen with a single reused generator (output-identical: generate()
    // is a method on the immutable, Send+Sync generator) ----
    {
        use llamacraft::worldgen::driver::ChunkGenerator;
        let mut reuse_ns = u128::MAX;
        for _ in 0..iters {
            let t = Instant::now();
            let g = ChunkGenerator::new(seed);
            let mut sink = 0u64;
            for &(cx, cz) in &coords {
                let region = g.region(cx, cz);
                let mut c = g.generate(&region, cx, cz);
                g.place_features(&mut c, &region);
                sink ^= c.block_raw(0, 64, 0) as u64;
            }
            std::hint::black_box(sink);
            reuse_ns = reuse_ns.min(t.elapsed().as_nanos());
        }
        println!(
            "wg(reuse): {:>8.2} ms total | {:>7.3} ms/chunk  (one generator)",
            reuse_ns as f64 / 1e6,
            reuse_ns as f64 / 1e6 / n as f64
        );
    }

    // ---- skylight bake (neighbor-aware per chunk, cached on the Chunk) ----
    // This is the expensive flood-fill, now done only when blocks/loaded borders
    // change in the real app instead of inside every mesh build.
    {
        let mut sky_ns = u128::MAX;
        for _ in 0..iters {
            let t = Instant::now();
            for &(cx, cz) in &coords {
                let (band, ylo, yhi) = {
                    let c = &chunks[&(cx, cz)];
                    compute_chunk_skylight_with_neighbors(c, |nx, nz| chunks.get(&(nx, nz)))
                };
                chunks
                    .get_mut(&(cx, cz))
                    .unwrap()
                    .set_skylight(band, ylo, yhi);
            }
            sky_ns = sky_ns.min(t.elapsed().as_nanos());
        }
        println!(
            "skylight : {:>8.2} ms total | {:>7.3} ms/chunk  (best of {})",
            sky_ns as f64 / 1e6,
            sky_ns as f64 / 1e6 / n as f64,
            iters
        );
    }

    // ---- meshing (samples cached skylight; real cross-chunk neighbour reads) ----
    // Mirrors world.rs: gather the 3x3 neighbourhood per chunk so light/cull reads
    // index an array instead of hashing the chunk map per voxel.
    let mut mesh_ns = u128::MAX;
    let mut total_quads = 0u64;
    for it in 0..iters {
        let t = Instant::now();
        let mut quads = 0u64;
        for &(cx, cz) in &coords {
            let c = &chunks[&(cx, cz)];
            let neigh: [Option<&Chunk>; 9] = std::array::from_fn(|k| {
                let dx = (k % 3) as i32 - 1;
                let dz = (k / 3) as i32 - 1;
                chunks.get(&(cx + dx, cz + dz))
            });
            let owner = |nx: i32, nz: i32| -> Option<&Chunk> {
                let (dcx, dcz) = (nx - cx, nz - cz);
                if (-1..=1).contains(&dcx) && (-1..=1).contains(&dcz) {
                    neigh[((dcz + 1) * 3 + (dcx + 1)) as usize]
                } else {
                    chunks.get(&(nx, nz))
                }
            };
            let block_at = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 || wy >= CHUNK_SY as i32 {
                    return 0;
                }
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.block_raw((wx & 15) as usize, wy as usize, (wz & 15) as usize),
                    None => 0,
                }
            };
            let biome_at = |wx: i32, wz: i32| -> u8 {
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.biome_at((wx & 15) as usize, (wz & 15) as usize),
                    None => 0,
                }
            };
            let light_at = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 {
                    return 0;
                }
                if wy >= CHUNK_SY as i32 {
                    return SKY_FULL;
                }
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.skylight_at((wx & 15) as usize, wy, (wz & 15) as usize),
                    None => SKY_FULL,
                }
            };
            let m = build_mesh(c, block_at, biome_at, light_at);
            quads += (m.opaque_idx.len() + m.transparent_idx.len()) as u64 / 6;
        }
        let e = t.elapsed().as_nanos();
        mesh_ns = mesh_ns.min(e);
        if it + 1 == iters {
            total_quads = quads;
        }
    }
    println!(
        "meshing  : {:>8.2} ms total | {:>7.3} ms/chunk  (best of {}), {} quads",
        mesh_ns as f64 / 1e6,
        mesh_ns as f64 / 1e6 / n as f64,
        iters,
        total_quads
    );

    // ---- bbmodel-block meshing: one chunk densely packed with model-block cells so the
    // bbmodel hot path (template instancing) is measured on its own, not drowned out by
    // terrain. A Y-band is filled with workbench cells, cycling the model's authored cell
    // offsets so every cell's template is exercised, then meshed many times. ----
    {
        use llamacraft::block::Block;
        use llamacraft::block_model::{instance, BlockModelKind};
        use llamacraft::furnace::Facing;

        let kind = BlockModelKind::FurnitureWorkbench;
        let offsets: Vec<[u8; 3]> = instance(kind).cells.iter().map(|c| c.offset).collect();
        let band = 64usize; // Y layers packed solid with model cells
        let mut mc = Chunk::new(0, 0);
        let mut n_cells = 0usize;
        for y in 1..(1 + band) {
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    mc.set_block(x, y, z, Block::FurnitureWorkbench);
                    mc.set_model_offset(x, y, z, offsets[n_cells % offsets.len()]);
                    mc.set_model_facing(x, y, z, Facing::North);
                    n_cells += 1;
                }
            }
        }

        let air = |_: i32, _: i32, _: i32| 0u8;
        let biome0 = |_: i32, _: i32| 0u8;
        let full = |_: i32, _: i32, _: i32| SKY_FULL;

        let model_quads = build_mesh(&mc, air, biome0, full).model_idx.len() as u64 / 6;
        let reps = 200usize;
        let mut model_ns = u128::MAX;
        for _ in 0..iters.max(1) {
            let t = Instant::now();
            let mut sink = 0u64;
            for _ in 0..reps {
                sink ^= build_mesh(&mc, air, biome0, full).model.len() as u64;
            }
            std::hint::black_box(sink);
            model_ns = model_ns.min(t.elapsed().as_nanos());
        }
        let per_mesh = model_ns as f64 / 1e6 / reps as f64;
        println!(
            "model    : {:>8.4} ms/chunk | {:>6.2} ns/quad  ({} cells, {} model-quads, best of {}×{})",
            per_mesh,
            per_mesh * 1e6 / model_quads.max(1) as f64,
            n_cells,
            model_quads,
            iters.max(1),
            reps,
        );
    }

    let _ = (CHUNK_SX, CHUNK_SZ);
}
