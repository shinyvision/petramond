//! Throwaway staged worldgen profiler. Times each generation stage separately
//! so we can attribute the per-chunk cost.
//!
//! Run: `cargo run --release --bin genprofile [grid_radius]`

use std::time::Instant;

use llamacraft::tooling::worldgen::ChunkGenerator;

fn main() {
    let r: i32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let seed = 0x1234_5678u32;
    let coords: Vec<(i32, i32)> = (-r..=r)
        .flat_map(|cz| (-r..=r).map(move |cx| (cx, cz)))
        .collect();
    let n = coords.len();

    let g = ChunkGenerator::new(seed);

    let mut surf_ns = 0u128;
    let mut under_ns = 0u128;
    let mut veg_ns = 0u128;
    let mut feat_ns = 0u128;
    let mut sink = 0u64;
    let mut worst = (0i32, 0i32, 0f64);

    for &(cx, cz) in &coords {
        let t0 = Instant::now();
        let mut chunk = g.generate_surface(cx, cz);
        let t1 = Instant::now();
        g.place_underground(&mut chunk);
        let t2 = Instant::now();
        g.place_vegetation(&mut chunk);
        let t3 = Instant::now();
        g.place_features_runtime(&mut chunk);
        let t4 = Instant::now();

        surf_ns += (t1 - t0).as_nanos();
        under_ns += (t2 - t1).as_nanos();
        veg_ns += (t3 - t2).as_nanos();
        feat_ns += (t4 - t3).as_nanos();

        let total_ms = (t4 - t0).as_nanos() as f64 / 1e6;
        if total_ms > worst.2 {
            worst = (cx, cz, total_ms);
        }
        sink ^= chunk.block_raw(0, 64, 0) as u64;
    }
    std::hint::black_box(sink);

    let per = |ns: u128| ns as f64 / 1e6 / n as f64;
    let total = surf_ns + under_ns + veg_ns + feat_ns;
    println!(
        "grid {}x{} = {} chunks, seed={:08x}",
        2 * r + 1,
        2 * r + 1,
        n,
        seed
    );
    println!(
        "surface    : {:>8.3} ms/chunk  ({:>5.1}%)",
        per(surf_ns),
        surf_ns as f64 / total as f64 * 100.0
    );
    println!(
        "underground: {:>8.3} ms/chunk  ({:>5.1}%)",
        per(under_ns),
        under_ns as f64 / total as f64 * 100.0
    );
    println!(
        "vegetation : {:>8.3} ms/chunk  ({:>5.1}%)",
        per(veg_ns),
        veg_ns as f64 / total as f64 * 100.0
    );
    println!(
        "features   : {:>8.3} ms/chunk  ({:>5.1}%)",
        per(feat_ns),
        feat_ns as f64 / total as f64 * 100.0
    );
    println!("TOTAL      : {:>8.3} ms/chunk", per(total));
    println!("worst chunk: ({},{}) = {:.1} ms", worst.0, worst.1, worst.2);
}
