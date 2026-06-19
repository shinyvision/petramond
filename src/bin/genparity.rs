//! Worldgen byte-parity gate (throwaway, used during the Strata refactor).
//!
//! Hashes the block + biome bytes of a fixed grid of chunks across several
//! seeds. Pure code-move phases (P0–P3) must reproduce the same COMBINED hash
//! as the baseline; phases that intentionally change output (P4) will not, and
//! are validated by screenshots instead.
//!
//! Run: `cargo run --quiet --bin genparity`

use llamacraft::worldgen::generate_chunk;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(bytes: &[u8], mut h: u64) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn main() {
    const SEEDS: [u32; 3] = [0x1234_5678, 1, 0xDEAD_BEEF];
    const COORDS: [(i32, i32); 9] = [
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, -1),
        (0, 0),
        (0, 1),
        (1, -1),
        (1, 0),
        (1, 1),
    ];

    let mut combined = FNV_OFFSET;
    for &seed in &SEEDS {
        for &(cx, cz) in &COORDS {
            let chunk = generate_chunk(seed, cx, cz);
            let mut h = FNV_OFFSET;
            h = fnv1a(chunk.blocks_slice(), h);
            h = fnv1a(chunk.biomes_slice(), h);
            println!("seed={seed:08x} cx={cx:>2} cz={cz:>2} hash={h:016x}");
            combined = fnv1a(&h.to_le_bytes(), combined);
        }
    }
    println!("COMBINED={combined:016x}");
}
