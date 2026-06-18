//! Headless top-down worldgen previewer (dev tool).
//!
//! Renders an NxN chunk region to a PNG by coloring each column's top block,
//! so worldgen output (biomes, forests, rivers, chunk seams) can be eyeballed
//! without the GPU app. Run: `cargo run --quiet --bin genmap [seed] [out.png]`

use llamacraft::block::Block;
use llamacraft::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use llamacraft::worldgen::generate_chunk;

/// Highest non-air block in a column + its Y.
fn top_block(c: &Chunk, x: usize, z: usize) -> (u8, i32) {
    for y in (0..CHUNK_SY).rev() {
        let b = c.block_raw(x, y, z);
        if b != 0 {
            return (b, y as i32);
        }
    }
    (0, 0)
}

fn color(block: u8, y: i32) -> [u8; 3] {
    let base = match Block::from_id(block) {
        Block::OakLeaves => [34, 102, 34],
        Block::OakLog => [110, 74, 38],
        Block::Grass => [78, 138, 58],
        Block::Sand => [214, 202, 150],
        Block::Snow => [236, 240, 246],
        Block::Stone => [130, 130, 134],
        Block::Water => [40, 92, 172],
        Block::Dirt => [122, 92, 62],
        _ => [12, 12, 12],
    };
    // Subtle height shading for relief.
    let shade = (0.72 + 0.006 * (y - 60) as f32).clamp(0.5, 1.18);
    [
        (base[0] as f32 * shade).clamp(0.0, 255.0) as u8,
        (base[1] as f32 * shade).clamp(0.0, 255.0) as u8,
        (base[2] as f32 * shade).clamp(0.0, 255.0) as u8,
    ]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let seed: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0x1234_5678);
    let out = args.next().unwrap_or_else(|| "/tmp/worldmap.png".to_string());

    let r: i32 = 10; // chunks each direction from origin
    let n = (r * 2) as usize; // chunks across
    let w = n * CHUNK_SX;
    let h = n * CHUNK_SZ;
    let mut buf = vec![0u8; w * h * 3];

    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, cx as i32 - r, cz as i32 - r);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let (b, y) = top_block(&chunk, x, z);
                    let col = color(b, y);
                    let px = (cz * CHUNK_SZ + z) * w + (cx * CHUNK_SX + x);
                    buf[px * 3..px * 3 + 3].copy_from_slice(&col);
                }
            }
        }
    }

    image::save_buffer(&out, &buf, w as u32, h as u32, image::ColorType::Rgb8)
        .expect("write worldmap png");
    println!("wrote {out} ({w}x{h}, seed {seed:#x})");
}
