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
    let mut cave_ns = 0u128;
    let mut under_ns = 0u128;
    let mut veg_ns = 0u128;
    let mut feat_ns = 0u128;
    let mut sink = 0u64;
    let mut worst = (0i32, 0i32, 0f64);

    for &(cx, cz) in &coords {
        let t0 = Instant::now();
        let mut chunk = g.generate_surface(cx, cz);
        let t1 = Instant::now();
        g.carve_caves(&mut chunk);
        let t2 = Instant::now();
        g.place_underground(&mut chunk);
        let t3 = Instant::now();
        g.place_vegetation(&mut chunk);
        let t4 = Instant::now();
        g.place_features_runtime(&mut chunk);
        let t5 = Instant::now();

        surf_ns += (t1 - t0).as_nanos();
        cave_ns += (t2 - t1).as_nanos();
        under_ns += (t3 - t2).as_nanos();
        veg_ns += (t4 - t3).as_nanos();
        feat_ns += (t5 - t4).as_nanos();

        let total_ms = (t5 - t0).as_nanos() as f64 / 1e6;
        if total_ms > worst.2 {
            worst = (cx, cz, total_ms);
        }
        sink ^= chunk.block_raw(0, 64, 0) as u64;
    }
    std::hint::black_box(sink);

    let per = |ns: u128| ns as f64 / 1e6 / n as f64;
    let total = surf_ns + cave_ns + under_ns + veg_ns + feat_ns;
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
        "caves      : {:>8.3} ms/chunk  ({:>5.1}%)",
        per(cave_ns),
        cave_ns as f64 / total as f64 * 100.0
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

    // --- Live per-section path: column data once, then each surface section (cy 0..16),
    //     exactly as the streamer drives it. This is what actually runs at runtime. ---
    let mut col_ns = 0u128;
    let mut sec_ns = 0u128;
    let mut sink2 = 0u64;
    let mut worst_sec = (0i32, 0i32, 0f64);
    for &(cx, cz) in &coords {
        let c0 = Instant::now();
        let col = g.generate_column_data(cx, cz);
        let c1 = Instant::now();
        for cy in 0..16 {
            sink2 ^= g.generate_section(&col, cy);
        }
        let c2 = Instant::now();
        col_ns += (c1 - c0).as_nanos();
        sec_ns += (c2 - c1).as_nanos();
        let total_ms = (c2 - c0).as_nanos() as f64 / 1e6;
        if total_ms > worst_sec.2 {
            worst_sec = (cx, cz, total_ms);
        }
    }
    std::hint::black_box(sink2);
    let sec_total = col_ns + sec_ns;
    println!();
    println!("=== LIVE per-section path (column_gen + 16 surface sections) ===");
    println!(
        "column data: {:>8.3} ms/col    ({:>5.1}%)",
        per(col_ns),
        col_ns as f64 / sec_total as f64 * 100.0
    );
    println!(
        "16 sections: {:>8.3} ms/col    ({:>5.1}%)",
        per(sec_ns),
        sec_ns as f64 / sec_total as f64 * 100.0
    );
    println!("TOTAL      : {:>8.3} ms/col", per(sec_total));
    println!(
        "worst col  : ({},{}) = {:.1} ms",
        worst_sec.0, worst_sec.1, worst_sec.2
    );
    println!(
        "per-section vs whole-column: {:.2}x",
        sec_total as f64 / total as f64
    );
}
