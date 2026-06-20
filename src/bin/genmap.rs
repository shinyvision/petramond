//! Headless worldgen previewer (dev tool).
//!
//! Renders a chunk region so worldgen output can be eyeballed without the GPU
//! app. Three modes:
//!   top   (default) — top-down, coloured by each column's top block.
//!   biome           — top-down, coloured by per-column biome id.
//!   side  <z>       — vertical cross-section at world Z = <z>, so overhangs,
//!                     ocean depth and mountain strata are visible.
//!
//! Run:
//!   cargo run --quiet --bin genmap -- [seed] [out.png] [mode] [arg]
//! e.g.
//!   cargo run --quiet --bin genmap -- 42 /tmp/top.png top
//!   cargo run --quiet --bin genmap -- 42 /tmp/biome.png biome
//!   cargo run --quiet --bin genmap -- 42 /tmp/cut.png side 0

use llamacraft::biome::{biome_at, Biome};
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

fn block_color(block: u8) -> [u8; 3] {
    match Block::from_id(block) {
        Block::OakLeaves => [44, 110, 40],
        Block::SpruceLeaves => [40, 78, 52],
        Block::BirchLeaves => [108, 150, 78],
        Block::JungleLeaves => [48, 124, 28],
        Block::AcaciaLeaves => [104, 130, 40],
        Block::DarkOakLeaves => [36, 84, 30],
        Block::MangroveLeaves => [60, 124, 44],
        Block::CherryLeaves => [236, 170, 206],
        Block::AzaleaLeaves => [88, 124, 56],
        Block::OakLog | Block::DarkOakLog => [104, 74, 40],
        Block::SpruceLog => [70, 50, 32],
        Block::BirchLog => [206, 206, 198],
        Block::JungleLog | Block::AcaciaLog => [120, 90, 54],
        Block::CherryLog => [76, 50, 60],
        Block::MangroveLog => [92, 44, 40],
        Block::Grass => [86, 140, 60],
        Block::Sand => [216, 204, 152],
        Block::RedSand => [190, 110, 56],
        Block::Sandstone => [222, 208, 156],
        Block::RedSandstone => [188, 108, 54],
        Block::Snow | Block::SnowBlock => [238, 242, 248],
        Block::PackedIce | Block::Ice => [160, 192, 232],
        Block::Stone => [128, 128, 132],
        Block::Granite => [150, 106, 90],
        Block::Diorite => [200, 200, 202],
        Block::Andesite => [136, 138, 138],
        Block::Tuff => [98, 102, 96],
        Block::Calcite => [224, 226, 222],
        Block::Water => [44, 96, 176],
        Block::Dirt | Block::CoarseDirt => [122, 92, 62],
        Block::Podzol => [92, 64, 32],
        Block::Mycelium => [126, 110, 120],
        Block::Gravel => [128, 122, 118],
        Block::Clay => [160, 166, 178],
        Block::Mud => [60, 52, 50],
        Block::MossBlock => [88, 110, 56],
        Block::Terracotta => [150, 92, 66],
        Block::WhiteTerracotta => [210, 186, 168],
        Block::OrangeTerracotta => [162, 84, 40],
        Block::YellowTerracotta => [186, 138, 50],
        Block::BrownTerracotta => [110, 76, 54],
        Block::RedTerracotta => [142, 70, 52],
        Block::LightGrayTerracotta => [150, 122, 110],
        Block::CoalOre => [54, 54, 56],
        Block::IronOre => [190, 160, 132],
        Block::CopperOre => [166, 120, 92],
        Block::GoldOre => [206, 184, 90],
        Block::RedstoneOre => [150, 60, 56],
        Block::LapisOre => [50, 84, 160],
        Block::DiamondOre => [110, 200, 206],
        Block::EmeraldOre => [70, 184, 110],
        Block::Cactus => [70, 120, 56],
        Block::Pumpkin => [200, 120, 40],
        Block::Melon => [90, 150, 60],
        Block::ShortGrass | Block::Fern => [86, 150, 58],
        Block::Dandelion => [220, 210, 70],
        Block::Poppy | Block::RedTulip => [200, 60, 50],
        Block::Cornflower => [90, 110, 210],
        Block::Allium => [170, 120, 200],
        Block::AzureBluet | Block::OxeyeDaisy => [225, 225, 220],
        Block::DeadBush => [150, 110, 60],
        Block::BrownMushroom => [150, 110, 80],
        Block::RedMushroom => [200, 70, 60],
        _ => [12, 12, 14],
    }
}

/// Distinct top-down colour per biome id.
fn biome_color(id: u8) -> [u8; 3] {
    match Biome::from_id(id) {
        Biome::Ocean => [46, 104, 180],
        Biome::DeepOcean => [20, 50, 116],
        Biome::Beach => [240, 232, 174], // pale sand
        Biome::River => [80, 150, 210],
        Biome::Desert => [224, 186, 88], // tan-orange (distinct from beach)
        Biome::Plains => [126, 198, 78], // bright green
        Biome::Savanna => [188, 186, 86], // olive
        Biome::Forest => [46, 128, 48],  // dark green
        Biome::BirchForest => [120, 176, 96],
        Biome::Wetland => [92, 144, 96],
        Biome::Swamp => [58, 92, 64],
        Biome::Taiga => [58, 116, 92],
        Biome::Foothills => [152, 168, 122], // gray-green
        Biome::Mountains => [138, 138, 132], // gray
        Biome::SnowyTundra => [210, 224, 228],
        Biome::SnowyTaiga => [168, 198, 198],
        Biome::SnowyPeaks => [238, 242, 250],
        Biome::Jungle => [40, 150, 30],
        Biome::Badlands => [184, 100, 48],
        Biome::DarkForest => [34, 72, 30],
        Biome::OldGrowthTaiga => [44, 96, 64],
        Biome::CherryGrove => [228, 150, 196],
        Biome::Meadow => [120, 200, 90],
        Biome::Grove => [184, 208, 208],
        Biome::SnowySlopes => [220, 230, 238],
        Biome::IceSpikes => [196, 224, 238],
        Biome::MushroomFields => [150, 110, 150],
        Biome::WindsweptHills => [130, 150, 120],
        Biome::StonyPeaks => [170, 168, 164],
    }
}

fn save(out: &str, buf: &[u8], w: usize, h: usize) {
    image::save_buffer(out, buf, w as u32, h as u32, image::ColorType::Rgb8).expect("write png");
}

/// Top-down map, coloured by either top block or biome id.
fn render_topdown(seed: u32, out: &str, by_biome: bool) {
    let r: i32 = 12;
    let n = (r * 2) as usize;
    let w = n * CHUNK_SX;
    let h = n * CHUNK_SZ;
    let mut buf = vec![0u8; w * h * 3];
    let mut heights: Vec<i32> = Vec::with_capacity(w * h);

    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, cx as i32 - r, cz as i32 - r);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let (b, y) = top_block(&chunk, x, z);
                    heights.push(y);
                    let mut col = if by_biome {
                        biome_color(chunk.biome_at(x, z))
                    } else {
                        block_color(b)
                    };
                    // Subtle height relief on the block map.
                    if !by_biome {
                        let shade = (0.66 + 0.007 * (y - 58) as f32).clamp(0.45, 1.25);
                        for c in &mut col {
                            *c = (*c as f32 * shade).clamp(0.0, 255.0) as u8;
                        }
                    }
                    let px = (cz * CHUNK_SZ + z) * w + (cx * CHUNK_SX + x);
                    buf[px * 3..px * 3 + 3].copy_from_slice(&col);
                }
            }
        }
    }
    save(out, &buf, w, h);
    heights.sort_unstable();
    let pct = |p: f64| heights[((heights.len() - 1) as f64 * p) as usize];
    let below_sea = heights.iter().filter(|&&y| y <= 64).count() as f64 / heights.len() as f64;
    println!(
        "wrote {out} ({w}x{h}, seed {seed:#x}, mode {})",
        if by_biome { "biome" } else { "top" }
    );
    println!(
        "  top-Y: min {} p10 {} p50 {} p90 {} p99 {} max {} | water-cover {:.0}%",
        heights[0],
        pct(0.10),
        pct(0.50),
        pct(0.90),
        pct(0.99),
        heights[heights.len() - 1],
        below_sea * 100.0
    );
}

/// Vertical cross-section at world Z = `slice_z`. `zoom` enlarges each voxel to
/// `zoom`×`zoom` px and centers the window on world X = `center_x` (so overhang
/// detail is legible); zoom=1 shows the full 512-wide strip centered at 0.
fn render_side(seed: u32, out: &str, slice_z: i32, zoom: usize, center_x: i32, proj: i32) {
    let zoom = zoom.max(1);
    let cells_x = 512 / zoom; // world blocks across
    let cells_y = (176 / zoom).clamp(80, 176); // world blocks tall
    let x0 = center_x - cells_x as i32 / 2;
    let w = cells_x * zoom;
    let h = cells_y * zoom;
    let mut buf = vec![0u8; w * h * 3];

    let cz = slice_z.div_euclid(CHUNK_SZ as i32);
    let lz = slice_z.rem_euclid(CHUNK_SZ as i32) as usize;

    // Auto-center the vertical window on the terrain at center_x.
    let y_top = {
        let ccx = center_x.div_euclid(CHUNK_SX as i32);
        let clx = center_x.rem_euclid(CHUNK_SX as i32) as usize;
        let cc = generate_chunk(seed, ccx, cz);
        let mut s = 64;
        for y in (0..CHUNK_SY).rev() {
            if cc.block_raw(clx, y, lz) != 0 {
                s = y as i32;
                break;
            }
        }
        // put the surface ~1/4 down from the top of the window
        (s + cells_y as i32 / 4).min(CHUNK_SY as i32 - 1)
    };
    let mut cur_cx = i32::MIN;
    let mut chunk: Option<Chunk> = None;

    for gx in 0..cells_x {
        let wx = x0 + gx as i32;
        let cx = wx.div_euclid(CHUNK_SX as i32);
        let lx = wx.rem_euclid(CHUNK_SX as i32) as usize;
        if cx != cur_cx {
            chunk = Some(generate_chunk(seed, cx, cz));
            cur_cx = cx;
        }
        let c = chunk.as_ref().unwrap();
        for gy in 0..cells_y {
            let wy = y_top - gy as i32; // row 0 = top
            let col = if wy < 0 || wy >= CHUNK_SY as i32 {
                [150, 190, 232]
            } else if proj > 0 {
                // Project over a z-window: prefer logs, then leaves, then terrain,
                // so a whole tree silhouette shows instead of one sliced plane.
                let (mut log, mut leaf, mut terr) = (false, false, 0u8);
                for dz in -proj..=proj {
                    let zz = lz as i32 + dz;
                    if zz < 0 || zz >= CHUNK_SZ as i32 {
                        continue; // stay within this chunk (fine for tree inspection)
                    }
                    let b = c.block_raw(lx, wy as usize, zz as usize);
                    match Block::from_id(b) {
                        Block::OakLog => log = true,
                        Block::OakLeaves => leaf = true,
                        Block::Air => {}
                        _ => terr = b,
                    }
                }
                if log {
                    block_color(Block::OakLog as u8)
                } else if leaf {
                    block_color(Block::OakLeaves as u8)
                } else if terr != 0 {
                    block_color(terr)
                } else if wy >= 64 {
                    [150, 190, 232]
                } else {
                    [22, 22, 26]
                }
            } else {
                let b = c.block_raw(lx, wy as usize, lz);
                if b == 0 {
                    if wy >= 64 {
                        [150, 190, 232]
                    } else {
                        [22, 22, 26]
                    }
                } else {
                    block_color(b)
                }
            };
            // expand to zoom×zoom
            for dy in 0..zoom {
                for dx in 0..zoom {
                    let px = (gy * zoom + dy) * w + (gx * zoom + dx);
                    buf[px * 3..px * 3 + 3].copy_from_slice(&col);
                }
            }
        }
    }
    save(out, &buf, w, h);
    println!("wrote {out} ({w}x{h}, seed {seed:#x}, side z={slice_z} zoom={zoom} cx={center_x})");
}

/// River diagnostic: top-down terrain with the active explicit river metadata
/// overlaid. Wet channel columns are blue, bank influence is amber, and any wet
/// channel column whose top block is not water is red for easy artifact spotting.
fn render_river(seed: u32, out: &str) {
    use llamacraft::worldgen::{driver::ChunkGenerator, generate_chunk_with};
    let generator = ChunkGenerator::new(seed);
    let r: i32 = 12;
    let n = (r * 2) as usize;
    let w = n * CHUNK_SX;
    let h = n * CHUNK_SZ;
    let mut buf = vec![0u8; w * h * 3];
    // Per-pixel masks for the island audit (built during the scan, used after).
    let mut water = vec![false; w * h];
    let mut bandmask = vec![false; w * h];
    let (mut band, mut channel, mut channel_water, mut centerline) = (0u64, 0u64, 0u64, 0u64);
    let mut total = 0u64;
    let mut dry_channel = 0u64;
    for cz in 0..n {
        for cx in 0..n {
            let gcx = cx as i32 - r;
            let gcz = cz as i32 - r;
            let region = generator.region(gcx, gcz);
            let chunk = generate_chunk_with(&generator, gcx, gcz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let wx = gcx * CHUNK_SX as i32 + x as i32;
                    let wz = gcz * CHUNK_SZ as i32 + z as i32;
                    let (b, _) = top_block(&chunk, x, z);
                    let river = region.river_at(wx, wz);
                    total += 1;
                    let carved = river.influence > 0.05;
                    let mut col = block_color(b);
                    if carved {
                        band += 1;
                        let is_water = Block::from_id(b) == Block::Water;
                        if river.distance < 0.75 {
                            centerline += 1;
                            col = [245, 248, 255];
                        } else if river.wet() {
                            channel += 1;
                            if is_water {
                                channel_water += 1;
                            }
                            let t = river.channel.clamp(0.0, 1.0);
                            col = [
                                (40.0 * (1.0 - t)) as u8,
                                (90.0 * (1.0 - t) + 90.0 * t) as u8,
                                (160.0 * (1.0 - t) + 255.0 * t) as u8,
                            ];
                            if !is_water {
                                dry_channel += 1;
                                col = [255, 24, 18];
                            }
                        } else {
                            let t = river.influence.clamp(0.0, 1.0);
                            col[0] = (col[0] as f32 * (1.0 - t) + 212.0 * t) as u8;
                            col[1] = (col[1] as f32 * (1.0 - t) + 156.0 * t) as u8;
                            col[2] = (col[2] as f32 * (1.0 - t) + 74.0 * t) as u8;
                        }
                    }
                    let px = (cz * CHUNK_SZ + z) * w + (cx * CHUNK_SX + x);
                    water[px] = Block::from_id(b) == Block::Water;
                    bandmask[px] = river.wet();
                    buf[px * 3..px * 3 + 3].copy_from_slice(&col);
                }
            }
        }
    }
    // Island audit: a wet-channel column that is LAND but has water within two
    // blocks on both opposite sides.
    let mut islands = 0u64;
    let near = |arr: &[bool], x: usize, y: usize, dx: i32, dy: i32| -> bool {
        for d in 1..=2 {
            let nx = x as i32 + dx * d;
            let ny = y as i32 + dy * d;
            if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                return false;
            }
            if arr[ny as usize * w + nx as usize] {
                return true;
            }
        }
        false
    };
    for y in 0..h {
        for x in 0..w {
            let px = y * w + x;
            if !bandmask[px] || water[px] {
                continue;
            }
            let enclosed_x = near(&water, x, y, 1, 0) && near(&water, x, y, -1, 0);
            let enclosed_z = near(&water, x, y, 0, 1) && near(&water, x, y, 0, -1);
            if enclosed_x || enclosed_z {
                islands += 1;
            }
        }
    }
    save(out, &buf, w, h);
    let p = |v: u64, d: u64| {
        if d > 0 {
            100.0 * v as f64 / d as f64
        } else {
            0.0
        }
    };
    println!("wrote {out} ({w}x{h}, seed {seed:#x}, mode river)");
    println!(
        "  river banks: {:.3}% of world | wet channel {:.3}% | top-is-water {:.1}% of channel",
        p(band, total),
        p(channel, total),
        p(channel_water, channel)
    );
    println!("  centerline pixels: {centerline} | dry wet-channel cols: {dry_channel}");
    println!("  island audit: mid-channel land cols enclosed by water: {islands}");
}

/// Print legacy `WorldNoise` field ranges. Active chunk terrain is generated by
/// the classic terrain provider; this mode is only for maintaining the remaining
/// legacy noise/cave diagnostics.
fn print_stats(seed: u32) {
    use llamacraft::worldgen::WorldNoise;
    let wn = WorldNoise::new(seed);
    println!("legacy WorldNoise stats for seed {seed:#x}");
    let (mut c, mut e, mut w, mut pv, mut j) = (vec![], vec![], vec![], vec![], vec![]);
    let (mut tp, mut hu) = (vec![], vec![]);
    let mut sy = vec![];
    let (mut mountain, mut foothill, mut rolling, mut plateau, mut wet_basin) =
        (vec![], vec![], vec![], vec![], vec![]);
    let mut biome_counts = [0u32; 29];
    let mut biome_grid = vec![];
    let mut z = -600;
    while z < 600 {
        let mut x = -600;
        while x < 600 {
            let (a, b, d, g) = wn.debug_sample(x, z);
            c.push(a);
            e.push(b);
            w.push(wn.debug_weirdness(x, z));
            pv.push(d);
            j.push(g);
            let cl = wn.climate(x, z);
            tp.push(cl.temperature as f64);
            hu.push(cl.humidity as f64);
            let surf = wn.surface_height(x, z);
            sy.push(surf as f64);
            let (m, f, r, p, wb) = wn.debug_landform(x, z);
            mountain.push(m as f64);
            foothill.push(f as f64);
            rolling.push(r as f64);
            plateau.push(p as f64);
            wet_basin.push(wb as f64);
            let bid = biome_at(cl, surf).id() as usize;
            if bid < biome_counts.len() {
                biome_counts[bid] += 1;
            }
            biome_grid.push(Biome::from_id(bid as u8));
            x += 5;
        }
        z += 5;
    }
    let stat = |v: &mut Vec<f64>, name: &str| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p = |q: f64| v[((v.len() - 1) as f64 * q) as usize];
        println!(
            "{name:8} min {:+.3} p05 {:+.3} p50 {:+.3} p95 {:+.3} max {:+.3}",
            v[0],
            p(0.05),
            p(0.50),
            p(0.95),
            v[v.len() - 1]
        );
    };
    stat(&mut c, "cont");
    stat(&mut e, "erosion");
    stat(&mut w, "weird");
    stat(&mut pv, "pv");
    stat(&mut j, "jagged");
    stat(&mut tp, "temp");
    stat(&mut hu, "humid");
    stat(&mut sy, "surf_y");
    stat(&mut mountain, "mountain");
    stat(&mut foothill, "foothill");
    stat(&mut rolling, "rolling");
    stat(&mut plateau, "plateau");
    stat(&mut wet_basin, "wetbasin");

    let total_cols = biome_counts.iter().sum::<u32>() as f64;
    let mut census: Vec<(f64, &str)> = (0..29u8)
        .map(|id| {
            let pct = 100.0 * biome_counts[id as usize] as f64 / total_cols;
            (pct, Biome::from_id(id).name())
        })
        .filter(|(p, _)| *p > 0.0)
        .collect();
    census.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let line: Vec<String> = census.iter().map(|(p, n)| format!("{n} {p:.1}%")).collect();
    println!("biomes   {}", line.join(", "));

    let grid_n = (1200 / 5) as usize;
    let mut components: Vec<(usize, &str)> = (0..29u8)
        .map(|id| {
            let biome = Biome::from_id(id);
            (
                largest_biome_component(&biome_grid, grid_n, biome),
                biome.name(),
            )
        })
        .filter(|(size, _)| *size > 0)
        .collect();
    components.sort_by(|a, b| b.0.cmp(&a.0));
    let line: Vec<String> = components
        .iter()
        .take(8)
        .map(|(size, name)| format!("{name} {size}"))
        .collect();
    println!(
        "largest  {} sampled cells @ 5-block stride",
        line.join(", ")
    );
}

fn largest_biome_component(grid: &[Biome], n: usize, target: Biome) -> usize {
    let mut seen = vec![false; grid.len()];
    let mut best = 0usize;
    let mut stack = Vec::new();

    for i in 0..grid.len() {
        if seen[i] || grid[i] != target {
            continue;
        }

        seen[i] = true;
        stack.push(i);
        let mut size = 0usize;
        while let Some(cur) = stack.pop() {
            size += 1;
            let x = cur % n;
            let z = cur / n;
            let push = |nx: usize, nz: usize, seen: &mut [bool], stack: &mut Vec<usize>| {
                let ni = nz * n + nx;
                if !seen[ni] && grid[ni] == target {
                    seen[ni] = true;
                    stack.push(ni);
                }
            };
            if x > 0 {
                push(x - 1, z, &mut seen, &mut stack);
            }
            if x + 1 < n {
                push(x + 1, z, &mut seen, &mut stack);
            }
            if z > 0 {
                push(x, z - 1, &mut seen, &mut stack);
            }
            if z + 1 < n {
                push(x, z + 1, &mut seen, &mut stack);
            }
        }
        best = best.max(size);
    }

    best
}

/// Audit overhangs + floating debris across a region. An "overhang ceiling" is a
/// solid voxel with air directly below it. A "floating" voxel is solid with NO
/// solid anywhere below it in its column (true detached debris — should be ~0).
/// Also reports the deepest ocean column and the tallest column's skin stack.
fn audit(seed: u32) {
    // Terrain-only solidity: exclude tree logs/leaves (they legitimately sit over
    // air gaps and would swamp the real terrain-overhang signal).
    let is_solid = |b: u8| {
        matches!(
            Block::from_id(b),
            Block::Stone | Block::Dirt | Block::Grass | Block::Sand | Block::Snow
        )
    };
    let r: i32 = 12;
    let n = (r * 2) as usize;
    let mut overhang = 0u64;
    let mut floating = 0u64;
    let mut deepest_floor = i32::MAX;
    let (mut tall, mut tall_chunk, mut tall_xz) = (0i32, (0, 0), (0usize, 0usize));
    let (mut best_oh, mut oh_loc) = (0u32, (0i32, 0i32));
    let mut biome_counts = [0u32; 29];
    let mut total_cols = 0u32;
    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, cx as i32 - r, cz as i32 - r);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let bid = chunk.biome_at(x, z) as usize;
                    if bid < 17 {
                        biome_counts[bid] += 1;
                    }
                    total_cols += 1;
                    // ocean floor depth (highest solid where water sits above)
                    let (tb, ty) = top_block(&chunk, x, z);
                    if Block::from_id(tb) == Block::Water {
                        let mut fy = 0;
                        for y in (0..CHUNK_SY).rev() {
                            if is_solid(chunk.block_raw(x, y, z)) {
                                fy = y as i32;
                                break;
                            }
                        }
                        deepest_floor = deepest_floor.min(fy);
                    } else if ty > tall {
                        tall = ty;
                        tall_chunk = (cx as i32 - r, cz as i32 - r);
                        tall_xz = (x, z);
                    }
                    // overhang + floating scan
                    let mut solid_below = false;
                    let mut col_oh = 0u32;
                    for y in 0..CHUNK_SY {
                        let s = is_solid(chunk.block_raw(x, y, z));
                        if s && y > 0 && !is_solid(chunk.block_raw(x, y - 1, z)) {
                            overhang += 1;
                            col_oh += 1;
                            if !solid_below {
                                floating += 1;
                            }
                        }
                        if s {
                            solid_below = true;
                        }
                    }
                    if col_oh > best_oh {
                        best_oh = col_oh;
                        oh_loc = (
                            (cx as i32 - r) * CHUNK_SX as i32 + x as i32,
                            (cz as i32 - r) * CHUNK_SZ as i32 + z as i32,
                        );
                    }
                }
            }
        }
    }
    // tallest column skin stack
    let tc = generate_chunk(seed, tall_chunk.0, tall_chunk.1);
    let (tx, tz) = tall_xz;
    let mut stack = String::new();
    for y in (tall - 6..=tall).rev() {
        if y < 0 {
            break;
        }
        let b = Block::from_id(tc.block_raw(tx, y as usize, tz));
        stack.push_str(&format!("y{y}:{b:?} "));
    }
    let twx = tall_chunk.0 * CHUNK_SX as i32 + tx as i32;
    let twz = tall_chunk.1 * CHUNK_SZ as i32 + tz as i32;
    println!("seed {seed:#x}: overhang-ceilings {overhang}  floating-debris {floating}  deepest-ocean-floor y{}  tallest y{tall} @ (x{twx},z{twz})", if deepest_floor == i32::MAX { -1 } else { deepest_floor });
    println!("  tallest skin: {stack}");
    println!(
        "  overhangiest column: {best_oh} ceilings @ (x{},z{})",
        oh_loc.0, oh_loc.1
    );
    let mut census: Vec<(f64, &str)> = (0..29u8)
        .map(|id| {
            let pct = 100.0 * biome_counts[id as usize] as f64 / total_cols as f64;
            (pct, Biome::from_id(id).name())
        })
        .filter(|(p, _)| *p > 0.0)
        .collect();
    census.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let line: Vec<String> = census.iter().map(|(p, n)| format!("{n} {p:.1}%")).collect();
    println!("  biomes: {}", line.join(", "));
}

/// True 3-D floating-debris census: build a region occupancy grid, flood-fill
/// upward from the bedrock layer (6-connected, across chunk boundaries), and count
/// solid terrain voxels NOT reachable from the bottom — genuine detached debris.
fn flood_audit(seed: u32) {
    let is_terrain = |b: u8| {
        matches!(
            Block::from_id(b),
            Block::Stone | Block::Dirt | Block::Grass | Block::Sand | Block::Snow
        )
    };
    let r: i32 = 8;
    let n = (r * 2) as usize;
    let w = n * CHUNK_SX;
    let hgt: usize = 190;
    let idx = |x: usize, y: usize, z: usize| (y * w + z) * w + x;
    let mut occ = vec![false; w * w * hgt];
    let mut solids: u64 = 0;
    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, cx as i32 - r, cz as i32 - r);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let gx = cx * CHUNK_SX + x;
                    let gz = cz * CHUNK_SZ + z;
                    for y in 0..hgt {
                        if is_terrain(chunk.block_raw(x, y, z)) {
                            occ[idx(gx, y, gz)] = true;
                            solids += 1;
                        }
                    }
                }
            }
        }
    }
    // flood from every solid in the bedrock layer (y=0)
    let mut reach = vec![false; w * w * hgt];
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    for z in 0..w {
        for x in 0..w {
            if occ[idx(x, 0, z)] {
                reach[idx(x, 0, z)] = true;
                stack.push((x, 0, z));
            }
        }
    }
    while let Some((x, y, z)) = stack.pop() {
        let push = |x: usize,
                    y: usize,
                    z: usize,
                    st: &mut Vec<(usize, usize, usize)>,
                    reach: &mut Vec<bool>| {
            let i = idx(x, y, z);
            if occ[i] && !reach[i] {
                reach[i] = true;
                st.push((x, y, z));
            }
        };
        if x + 1 < w {
            push(x + 1, y, z, &mut stack, &mut reach);
        }
        if x > 0 {
            push(x - 1, y, z, &mut stack, &mut reach);
        }
        if z + 1 < w {
            push(x, y, z + 1, &mut stack, &mut reach);
        }
        if z > 0 {
            push(x, y, z - 1, &mut stack, &mut reach);
        }
        if y + 1 < hgt {
            push(x, y + 1, z, &mut stack, &mut reach);
        }
        if y > 0 {
            push(x, y - 1, z, &mut stack, &mut reach);
        }
    }
    let mut floaters: u64 = 0;
    for i in 0..occ.len() {
        if occ[i] && !reach[i] {
            floaters += 1;
        }
    }
    let ppm = floaters as f64 / solids as f64 * 1_000_000.0;
    println!(
        "seed {seed:#x}: solids {solids}  detached-debris {floaters} ({ppm:.1} ppm of solid terrain), region {w}x{w}x{hgt}"
    );
}

/// Hillshaded top-down relief: colour each column by its top block, then light it
/// by the surface-height gradient (Lambert against a NW light). Jagged terrain
/// shows as sharp speckled relief; smooth domes show as soft gradients — so this
/// directly reveals whether mountains are craggy ranges or "boobs".
fn render_shaded(
    seed: u32,
    out: &str,
    center_x: i32,
    center_z: i32,
    radius_chunks: i32,
    px_scale: i32,
) {
    let r = radius_chunks;
    let s = px_scale.max(1) as usize; // upscale: each world column -> s×s pixels
    let n = (r * 2) as usize;
    let w = n * CHUNK_SX;
    let h = n * CHUNK_SZ;
    let ccx = center_x.div_euclid(CHUNK_SX as i32);
    let ccz = center_z.div_euclid(CHUNK_SZ as i32);
    let mut top = vec![0u8; w * h]; // block id
    let mut hgt = vec![0i32; w * h];
    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, ccx + cx as i32 - r, ccz + cz as i32 - r);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let (b, y) = top_block(&chunk, x, z);
                    let px = (cz * CHUNK_SZ + z) * w + (cx * CHUNK_SX + x);
                    top[px] = b;
                    hgt[px] = y;
                }
            }
        }
    }
    let at = |x: i32, z: i32| -> i32 {
        let x = x.clamp(0, w as i32 - 1) as usize;
        let z = z.clamp(0, h as i32 - 1) as usize;
        hgt[z * w + x]
    };
    let (pw, ph) = (w * s, h * s);
    let mut buf = vec![0u8; pw * ph * 3];
    for z in 0..h as i32 {
        for x in 0..w as i32 {
            // Surface-gradient hillshade (NW light), exaggerated for relief.
            let dzx = (at(x + 1, z) - at(x - 1, z)) as f32 * 0.5;
            let dzz = (at(x, z + 1) - at(x, z - 1)) as f32 * 0.5;
            let nx = -dzx;
            let nz = -dzz;
            let ny = 1.0;
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            // light from NW, fairly low so slopes catch it
            let (lx, ly, lz) = (-0.55f32, 0.62, -0.55);
            let lambert = ((nx * lx + ny * ly + nz * lz) / len).clamp(0.0, 1.0);
            let shade = 0.35 + 0.95 * lambert; // ambient + diffuse
            let base = block_color(top[(z as usize) * w + x as usize]);
            let mut col = base;
            for c in &mut col {
                *c = (*c as f32 * shade).clamp(0.0, 255.0) as u8;
            }
            // splat the column into an s×s pixel block
            for dy in 0..s {
                for dx in 0..s {
                    let pxp = (z as usize * s + dy) * pw + (x as usize * s + dx);
                    buf[pxp * 3..pxp * 3 + 3].copy_from_slice(&col);
                }
            }
        }
    }
    save(out, &buf, pw, ph);
    println!("wrote {out} ({pw}x{ph}, seed {seed:#x}, shaded @ ({center_x},{center_z}) r{radius_chunks}ch x{s})");
}

/// Walkability / spikiness metric. For "mountain" columns (top solid above y90)
/// reports how steep the surface is between neighbours — the thing cross-sections
/// and hillshades HIDE but that turns a mountain into a field of 1-wide pillars.
/// `pillar%` = columns that stick >=4 above ALL four neighbours (isolated spikes);
/// `walkable%` = columns whose steepest neighbour step is <=2 (you can stand/walk).
fn roughness(seed: u32) {
    let r: i32 = 12;
    let n = (r * 2) as usize;
    let w = n * CHUNK_SX;
    let mut surf = vec![0i32; w * w];
    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, cx as i32 - r, cz as i32 - r);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let (_, y) = top_block(&chunk, x, z);
                    surf[(cz * CHUNK_SZ + z) * w + (cx * CHUNK_SX + x)] = y;
                }
            }
        }
    }
    let at = |x: i32, z: i32| {
        surf[(z.clamp(0, w as i32 - 1) as usize) * w + x.clamp(0, w as i32 - 1) as usize]
    };
    let (mut mtn, mut pillars, mut walkable) = (0u64, 0u64, 0u64);
    let mut step_sum = 0i64;
    let mut steps_hist = [0u64; 6]; // 0,1,2,3,4,5+ block max-step buckets
    for z in 1..w as i32 - 1 {
        for x in 1..w as i32 - 1 {
            let h = at(x, z);
            if h <= 90 {
                continue;
            }
            mtn += 1;
            let nb = [at(x + 1, z), at(x - 1, z), at(x, z + 1), at(x, z - 1)];
            let max_step = nb.iter().map(|&v| (h - v).abs()).max().unwrap();
            let above_all = nb.iter().all(|&v| h - v >= 4);
            step_sum += max_step as i64;
            steps_hist[(max_step.min(5)) as usize] += 1;
            if above_all {
                pillars += 1;
            }
            if max_step <= 2 {
                walkable += 1;
            }
        }
    }
    if mtn == 0 {
        println!("seed {seed:#x}: no mountain columns (>y90) in region");
        return;
    }
    let pct = |v: u64| 100.0 * v as f64 / mtn as f64;
    println!(
        "seed {seed:#x}: mtn-cols {mtn}  mean-max-step {:.2}  pillar {:.1}%  walkable {:.1}%",
        step_sum as f64 / mtn as f64,
        pct(pillars),
        pct(walkable)
    );
    println!(
        "  max-step hist (0,1,2,3,4,5+): {:.0}% {:.0}% {:.0}% {:.0}% {:.0}% {:.0}%",
        pct(steps_hist[0]),
        pct(steps_hist[1]),
        pct(steps_hist[2]),
        pct(steps_hist[3]),
        pct(steps_hist[4]),
        pct(steps_hist[5])
    );
}

/// Oblique 3-D heightfield render (Comanche/voxel-landscape style). This is the
/// view a PLAYER sees — the one cross-sections and hillshades hid. Camera at
/// `(cam_x, cam_y, cam_z)` looks in -Z; for each screen column a ray marches the
/// per-column surface height front-to-back, projecting it to screen and painting
/// vertical spans (painter's algorithm). Pillars/needles and walkable surfaces
/// look exactly as in-game. (Heightfield only — overhang undersides aren't drawn.)
fn render_view(seed: u32, out: &str, cam_x: i32, cam_y: i32, cam_z: i32, scale: f32) {
    use std::collections::HashMap;
    let (w, h) = (640usize, 360usize);
    let horizon = (h as f32) * 0.45;
    let scale = if scale > 0.0 { scale } else { 150.0 }; // vertical proj / focal length
    let fov = 0.7f32; // half-width spread per unit distance
    let max_d = 420i32;

    let mut cache: HashMap<(i32, i32), Chunk> = HashMap::new();
    let surf = |wx: i32, wz: i32, cache: &mut HashMap<(i32, i32), Chunk>| -> (u8, i32) {
        let cx = wx.div_euclid(CHUNK_SX as i32);
        let cz = wz.div_euclid(CHUNK_SZ as i32);
        let c = cache
            .entry((cx, cz))
            .or_insert_with(|| generate_chunk(seed, cx, cz));
        let lx = wx.rem_euclid(CHUNK_SX as i32) as usize;
        let lz = wz.rem_euclid(CHUNK_SZ as i32) as usize;
        top_block(c, lx, lz)
    };

    // Auto-frame: scan the frustum for the tallest column, sit the camera below it
    // so the peak lands in the upper third regardless of the region's scale.
    let mut peak = 64;
    for d in (20..max_d).step_by(4) {
        let half = fov * d as f32;
        let wz = cam_z - d;
        for k in 0..40 {
            let wx = cam_x as f32 + (-half + 2.0 * half * (k as f32 / 39.0));
            peak = peak.max(surf(wx.round() as i32, wz, &mut cache).1);
        }
    }
    let cam_y = (peak - 58).max(cam_y).max(70); // passed cam_y is a floor

    let mut buf = vec![0u8; w * h * 3];
    // sky
    for px in buf.chunks_mut(3) {
        px.copy_from_slice(&[150, 190, 232]);
    }
    let mut ybuf = vec![h as f32; w]; // lowest sky-free y per column (start at bottom)
                                      // Front-to-back: nearer distances drawn first own the column top.
    for d in 1..max_d {
        let df = d as f32;
        let half = fov * df;
        let wz = cam_z - d;
        for sx in 0..w {
            let t = sx as f32 / (w - 1) as f32;
            let wx = cam_x as f32 + (-half + 2.0 * half * t);
            let (b, hy) = surf(wx.round() as i32, wz, &mut cache);
            let sy = horizon + (cam_y as f32 - hy as f32) * scale / df;
            let sy = sy.clamp(0.0, h as f32);
            if sy < ybuf[sx] {
                // simple depth + slope shade
                let shade = (1.15 - df / (max_d as f32) * 0.85).clamp(0.25, 1.0);
                let mut col = block_color(b);
                for c in &mut col {
                    *c = (*c as f32 * shade) as u8;
                }
                let y0 = sy.max(0.0) as usize;
                let y1 = ybuf[sx] as usize;
                for y in y0..y1 {
                    let p = (y * w + sx) * 3;
                    buf[p..p + 3].copy_from_slice(&col);
                }
                ybuf[sx] = sy;
            }
        }
    }
    save(out, &buf, w, h);
    println!("wrote {out} ({w}x{h}, seed {seed:#x}, view cam=({cam_x},{cam_y},{cam_z}))");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let seed: u32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x1234_5678);
    let out = args
        .next()
        .unwrap_or_else(|| "/tmp/worldmap.png".to_string());
    let mode = args.next().unwrap_or_else(|| "top".to_string());
    let arg = args.next().and_then(|s| s.parse::<i32>().ok());
    let zoom = args
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);
    let center_x = args.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
    let proj = args.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);

    match mode.as_str() {
        "biome" => render_topdown(seed, &out, true),
        "side" => render_side(seed, &out, arg.unwrap_or(0), zoom, center_x, proj),
        "shade" => render_shaded(
            seed,
            &out,
            center_x,
            arg.unwrap_or(0),
            zoom.max(1) as i32,
            proj,
        ),
        "stats" => print_stats(seed),
        "audit" => audit(seed),
        "rough" => roughness(seed),
        "view" => render_view(
            seed,
            &out,
            center_x,
            zoom as i32,
            arg.unwrap_or(0),
            proj as f32,
        ),
        "flood" => flood_audit(seed),
        "river" => render_river(seed, &out),
        "bench" => bench(seed, arg.unwrap_or(24)),
        _ => render_topdown(seed, &out, false),
    }
}

/// Time the game per-chunk generation path (one reused generator, like the
/// worker), reporting chunks/sec over a `radius`-chunk square.
fn bench(seed: u32, radius: i32) {
    use llamacraft::worldgen::{driver::ChunkGenerator, generate_chunk_with};
    let generator = ChunkGenerator::new(seed);
    // Warm up (touch a few chunks) so allocation/codepaths are hot.
    for cz in -1..=1 {
        for cx in -1..=1 {
            let _ = generate_chunk_with(&generator, cx, cz);
        }
    }
    let n = (2 * radius + 1) * (2 * radius + 1);
    let t0 = std::time::Instant::now();
    let mut acc = 0u64;
    let (mut t_gen, mut t_ug, mut t_veg, mut t_feat) = (0.0f64, 0.0, 0.0, 0.0);
    for cz in -radius..=radius {
        for cx in -radius..=radius {
            let region = generator.region(cx, cz);
            let s = std::time::Instant::now();
            let mut chunk = generator.generate(&region, cx, cz);
            t_gen += s.elapsed().as_secs_f64();
            let s = std::time::Instant::now();
            generator.place_underground(&mut chunk);
            t_ug += s.elapsed().as_secs_f64();
            let s = std::time::Instant::now();
            generator.place_vegetation(&mut chunk);
            t_veg += s.elapsed().as_secs_f64();
            let s = std::time::Instant::now();
            generator.place_features(&mut chunk, &region);
            t_feat += s.elapsed().as_secs_f64();
            acc = acc.wrapping_add(chunk.blocks_slice()[0] as u64);
        }
    }
    let dt = t0.elapsed();
    let per = dt.as_secs_f64() * 1000.0 / n as f64;
    let ms = |t: f64| t * 1000.0 / n as f64;
    println!(
        "bench: {n} chunks {:.3}s = {:.3} ms/chunk ({:.0}/s) [acc {acc}]",
        dt.as_secs_f64(),
        per,
        n as f64 / dt.as_secs_f64()
    );
    println!(
        "  generate {:.3}  underground {:.3}  vegetation {:.3}  features {:.3}  ms/chunk",
        ms(t_gen),
        ms(t_ug),
        ms(t_veg),
        ms(t_feat)
    );
    let _ = generate_chunk_with;
}
