//! Worldgen pipeline — **Strata**.
//!
//! `generate_chunk(seed, cx, cz) -> Chunk` is the single deterministic
//! entrypoint, invoked in isolation on a worker thread (native pool / web
//! Worker) and serialized to flat block + per-column biome bytes.
//!
//! Strata P0: this module is the relocated `gen.rs` god file, split verbatim
//! into submodules behind an unchanged public ABI (`generate_chunk`,
//! `WorldNoise`). `src/gen.rs` is now a thin re-export shim. Subsequent phases
//! decompose each concern into its own subsystem (see
//! `docs/ARCHITECTURE_WORLDGEN.md`):
//!   - P1: `noise` -> typed `HeightField` + const sampler settings
//!   - P2: a staged driver + `ProtoChunk` + declarative surface rules + carvers
//!   - P3: a composable `feature` system (trunk/foliage placers) replacing `trees`
//!   - P4: cross-chunk margin + positional RNG + data-driven biome definitions

pub mod noise;
pub mod rng;
pub mod trees;
pub mod features;

pub use noise::WorldNoise;

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
use crate::biome::{Biome, biome_at};

// ---------- biome blocks ----------

/// Pick block for the top solid surface given biome + height + river state.
pub fn surface_block(b: Biome, y: i32, river: f32) -> Block {
    if river > 0.05 && y <= SEA_LEVEL + 1 {
        return Block::Sand;
    }
    match b {
        Biome::Ocean => Block::Sand,
        Biome::Beach => Block::Sand,
        Biome::Desert => Block::Sand,
        Biome::Plains => Block::Grass,
        Biome::Forest => Block::Grass,
        Biome::BirchForest => Block::Grass,
        Biome::Savanna => Block::Grass,
        Biome::Swamp => Block::Grass,
        Biome::Taiga => Block::Grass,
        Biome::SnowyTundra => Block::Snow,
        Biome::SnowyTaiga => Block::Snow,
        Biome::Mountains => {
            if y > 95 { Block::Snow }
            else if y > 78 { Block::Stone }
            else { Block::Grass }
        }
        Biome::SnowyPeaks => Block::Snow,
        Biome::River => Block::Sand,
    }
}

pub fn subsurface_block(b: Biome) -> Block {
    match b {
        Biome::Desert => Block::Sand,
        Biome::Beach => Block::Sand,
        Biome::Mountains if true => Block::Stone, // below surface in mtns
        Biome::SnowyPeaks => Block::Stone,
        _ => Block::Dirt,
    }
}

/// Build column block stack for (x,z) into chunk buffer.
pub fn build_column(noise: &WorldNoise, chunk: &mut Chunk, x: usize, z: usize) {
    let (wx, wz) = {
        let (ox, oz) = chunk.chunk_origin_world();
        (ox + x as i32, oz + z as i32)
    };
    let surf = noise.surface_height(wx, wz);
    let climate = noise.climate(wx, wz);
    let biome = biome_at(climate, surf);
    let river = noise.river_strength(wx, wz);

    let top = surface_block(biome, surf, river);
    let sub = subsurface_block(biome);

    chunk.set_biome(x, z, biome.id());

    let carve = river > 0.05;
    let river_bed_y = (SEA_LEVEL - 2).max(surf - 4);

    for y in 0..CHUNK_SY {
        let y = y as i32;
        let b = if y > surf {
            if y <= SEA_LEVEL { Block::Water } else { Block::Air }
        } else if carve && y >= river_bed_y && y <= SEA_LEVEL {
            if y <= SEA_LEVEL { Block::Water } else { Block::Air }
        } else if carve && y == river_bed_y - 1 {
            sub
        } else if y == surf {
            top
        } else if y > surf - 5 {
            sub
        } else {
            Block::Stone
        };
        if b != Block::Air {
            chunk.set_block_raw(x, y as usize, z, b.id());
        }
    }
}

/// Generate terrain + features for a chunk. Caller passes seed.
pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
    let mut chunk = Chunk::new(cx, cz);
    let noise = WorldNoise::new(seed);

    // Terrain columns.
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            build_column(&noise, &mut chunk, x, z);
        }
    }

    // Trees & features layered on top via deterministic RNG seeded per chunk.
    let mut rng = crate::worldgen::rng::FeatureRng::new(seed, cx, cz);
    crate::worldgen::features::place_features(&mut chunk, &noise, &mut rng);

    chunk.dirty = true;
    chunk
}
