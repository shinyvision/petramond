// probe: distribution of max heightmap over a chunk grid
use llamacraft::chunk::{CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use llamacraft::worldgen::generate_chunk;

fn main() {
    let r: i32 = 8;
    let seed = 0x1234_5678u32;
    let coords: Vec<(i32, i32)> = (-r..=r)
        .flat_map(|cz| (-r..=r).map(move |cx| (cx, cz)))
        .collect();
    let n = coords.len();
    let mut sum_maxh = 0u64;
    let mut min_maxh = u16::MAX;
    let mut max_maxh = 0u16;
    let mut sum_volume_scanned_full = 0u64;
    let mut sum_volume_scanned_bounded = 0u64;
    for &(cx, cz) in &coords {
        let c = generate_chunk(seed, cx, cz);
        let mut mh = 0u16;
        for &h in c.heightmap.iter() {
            if h > mh {
                mh = h;
            }
        }
        sum_maxh += mh as u64;
        min_maxh = min_maxh.min(mh);
        max_maxh = max_maxh.max(mh);
        sum_volume_scanned_full += (CHUNK_SY * CHUNK_SX * CHUNK_SZ) as u64;
        sum_volume_scanned_bounded += ((mh as usize + 1) * CHUNK_SX * CHUNK_SZ) as u64;
    }
    println!(
        "chunks={} avg_maxh={:.1} min={} max={}",
        n,
        sum_maxh as f64 / n as f64,
        min_maxh,
        max_maxh
    );
    println!("full voxels scanned   = {}", sum_volume_scanned_full);
    println!(
        "bounded voxels scanned= {} ({:.1}% of full)",
        sum_volume_scanned_bounded,
        100.0 * sum_volume_scanned_bounded as f64 / sum_volume_scanned_full as f64
    );
    let _ = (CHUNK_SX, CHUNK_SZ);
}
