//! Worldgen byte-parity gate.
//!
//! Hashes the block + biome bytes of a fixed spread of chunks across several
//! seeds into one COMBINED hash. A pure code move must reproduce the hash of
//! the tree it started from; a change that means to alter output will not, and
//! is judged by captures instead.
//!
//! Run: `cargo run --quiet --profile playtest --bin genparity`

use petramond_worldgen::generate_chunk;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a_u16(cells: &[u16], mut h: u64) -> u64 {
    for &c in cells {
        h ^= c as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

fn fnv1a(bytes: &[u8], mut h: u64) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn main() {
    // A wide grid at several seeds so forests, wooded hills, riverbank plains
    // and redwood stands all fall inside the sample.
    const SEEDS: [u32; 3] = [0x1234_5678, 786, 0xDEAD_BEEF];
    let mut combined = FNV_OFFSET;
    for &seed in &SEEDS {
        for cz in -12..=12 {
            for cx in -12..=12 {
                if (cx + cz) % 3 != 0 {
                    continue;
                }
                let chunk = generate_chunk(seed, cx * 5, cz * 5);
                let mut h = FNV_OFFSET;
                h = fnv1a_u16(chunk.blocks_slice(), h);
                h = fnv1a(chunk.biomes_slice(), h);
                combined = fnv1a(&h.to_le_bytes(), combined);
            }
        }
    }
    println!("COMBINED={combined:016x}");
}
