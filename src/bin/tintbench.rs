// Standalone microbench of the tint precompute vs full mesh, to estimate the share.
use llamacraft::biome::Biome;
use llamacraft::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use llamacraft::worldgen::generate_chunk;
use std::collections::HashMap;
use std::time::Instant;

fn main() {
    let r: i32 = 8;
    let seed = 0x1234_5678u32;
    let coords: Vec<(i32, i32)> = (-r..=r)
        .flat_map(|cz| (-r..=r).map(move |cx| (cx, cz)))
        .collect();
    let mut chunks: HashMap<(i32, i32), Chunk> = HashMap::new();
    for &(cx, cz) in &coords {
        chunks.insert((cx, cz), generate_chunk(seed, cx, cz));
    }
    let n = coords.len();

    let biome_at = |wx: i32, wz: i32| -> u8 {
        let (cx, cz) = (wx >> 4, wz >> 4);
        match chunks.get(&(cx, cz)) {
            Some(c) => c.biome_at((wx & 15) as usize, (wz & 15) as usize),
            None => 0,
        }
    };

    // Replicate the tint precompute exactly.
    let mut best = u128::MAX;
    let mut sink = 0f32;
    let mut uniform_cols = 0u64;
    let mut total_cols = 0u64;
    let mut need_grass = 0u64;
    let mut need_fol = 0u64;
    let mut need_water = 0u64;
    let mut need_none = 0u64;
    for _ in 0..5 {
        let t = Instant::now();
        for &(cx, cz) in &coords {
            let ox = cx * CHUNK_SX as i32;
            let oz = cz * CHUNK_SZ as i32;
            const R: i32 = 2;
            let nf = (2 * R + 1) as f32 * (2 * R + 1) as f32;
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    let mut g = [0f32; 3];
                    let mut f = [0f32; 3];
                    let mut w = [0f32; 3];
                    for dz in -R..=R {
                        for dx in -R..=R {
                            let b = Biome::from_id(biome_at(wx + dx, wz + dz));
                            g[0] += b.grass_color()[0];
                            g[1] += b.grass_color()[1];
                            g[2] += b.grass_color()[2];
                            f[0] += b.foliage_color()[0];
                            f[1] += b.foliage_color()[1];
                            f[2] += b.foliage_color()[2];
                            w[0] += b.water_color()[0];
                            w[1] += b.water_color()[1];
                            w[2] += b.water_color()[2];
                        }
                    }
                    sink += g[0] / nf + f[1] / nf + w[2] / nf;
                }
            }
        }
        best = best.min(t.elapsed().as_nanos());
    }
    std::hint::black_box(sink);
    println!(
        "tint precompute alone: {:>7.3} ms total | {:>7.4} ms/chunk (best of 5)",
        best as f64 / 1e6,
        best as f64 / 1e6 / n as f64
    );

    // Coherence + need stats over the grid.
    for &(cx, cz) in &coords {
        let ox = cx * CHUNK_SX as i32;
        let oz = cz * CHUNK_SZ as i32;
        // need flags: scan block ids in chunk
        let c = &chunks[&(cx, cz)];
        let (mut ng, mut nl, mut nw) = (false, false, false);
        for y in 0..CHUNK_SY {
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    match c.block_raw(x, y, z) {
                        1 => ng = true, // Grass
                        6 => nw = true, // Water
                        8 => nl = true, // OakLeaves
                        _ => {}
                    }
                }
            }
        }
        if ng {
            need_grass += 1;
        }
        if nl {
            need_fol += 1;
        }
        if nw {
            need_water += 1;
        }
        if !ng && !nl && !nw {
            need_none += 1;
        }
        const R: i32 = 2;
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                total_cols += 1;
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let first = biome_at(wx - R, wz - R);
                let mut uni = true;
                'o: for dz in -R..=R {
                    for dx in -R..=R {
                        if biome_at(wx + dx, wz + dz) != first {
                            uni = false;
                            break 'o;
                        }
                    }
                }
                if uni {
                    uniform_cols += 1;
                }
            }
        }
    }
    println!(
        "chunks needing grass={}/{} foliage={}/{} water={}/{} none={}/{}",
        need_grass, n, need_fol, n, need_water, n, need_none, n
    );
    println!(
        "uniform-biome columns: {}/{} = {:.1}%",
        uniform_cols,
        total_cols,
        100.0 * uniform_cols as f64 / total_cols as f64
    );
}
