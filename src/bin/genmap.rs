//! Headless worldgen previewer (dev tool).
//!
//! Renders a chunk region so worldgen output can be eyeballed without the GPU
//! app. Main modes:
//!   top   (default) — top-down, coloured by each column's top block.
//!   biome           — top-down, coloured by per-column biome id.
//!   side  <z>       — vertical cross-section at world Z = <z>, so overhangs,
//!                     ocean depth and mountain strata are visible (y 0..255).
//!   deep  <z>       — full-depth cross-section (y -64..255) from the cubic
//!                     per-section generator: deep caves, marble, ores.
//!   cavestats       — cave/ore census: carved share per depth band, marble
//!                     share, ore counts per chunk, entrance-mouth rate.
//!
//! Run:
//!   cargo run --quiet --bin genmap -- [seed] [out.png] [mode] [arg]
//! e.g.
//!   cargo run --quiet --bin genmap -- 42 /tmp/top.png top
//!   cargo run --quiet --bin genmap -- 42 /tmp/biome.png biome
//!   cargo run --quiet --bin genmap -- 42 /tmp/cut.png side 0

use petramond::tooling::atlas::TileTint;
use petramond::tooling::biome::Biome;
use petramond::tooling::block::Block;
use petramond::tooling::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use petramond::tooling::worldgen::{generate_chunk, macro_surface_map};

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

/// Top-down colour for a block, derived from its data row: the top tile's
/// cartography average ([`Tile::map_rgb`](petramond::tooling::atlas::Tile::map_rgb)),
/// with tinted tiles (grass/foliage/water) multiplied by a fixed Plains biome
/// colour — the previewer has no per-column tint blend and doesn't need one.
fn block_color(block: u8) -> [u8; 3] {
    let b = Block::from_id(block);
    if b == Block::Air {
        return [12, 12, 14];
    }
    let tile = b.tiles()[0];
    let base = tile.map_rgb();
    let tint = match tile.world_tint() {
        None => return base,
        Some(TileTint::Grass) => Biome::Plains.grass_color(),
        Some(TileTint::Foliage) => Biome::Plains.foliage_color(),
        Some(TileTint::Water) => Biome::Plains.water_color(),
    };
    std::array::from_fn(|c| (base[c] as f32 * tint[c]).round().clamp(0.0, 255.0) as u8)
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
        Biome::RedwoodForest => [30, 90, 36], // deep dark green — tall redwood groves
        Biome::Wetland => [92, 144, 96],
        Biome::Swamp => [58, 92, 64],
        Biome::Taiga => [58, 116, 92],
        Biome::Foothills => [152, 168, 122], // gray-green
        Biome::Mountains => [138, 138, 132], // gray
        Biome::SnowyTundra => [210, 224, 228],
        Biome::SnowyTaiga => [168, 198, 198],
        Biome::SnowyPeaks => [238, 242, 250],
        Biome::OldGrowthTaiga => [48, 96, 70],
        Biome::Meadow => [150, 210, 104],
        Biome::Grove => [150, 180, 172],
        Biome::SnowySlopes => [224, 232, 238],
        Biome::WindsweptHills => [126, 138, 126],
        Biome::StonyPeaks => [166, 166, 160],
        Biome::WoodedHills => [64, 132, 56],
        Biome::MountainEdge => [148, 158, 132],
        Biome::DesertLakes => [214, 178, 92],
        Biome::SnowyPlains => [222, 230, 226],
    }
}

fn save(out: &str, buf: &[u8], w: usize, h: usize) {
    // Timing/stat runs pass /dev/null; skip the encode (the image crate can't
    // infer a format from it) but keep the printed summaries.
    if out == "/dev/null" {
        return;
    }
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
    let mut gen_time = std::time::Duration::ZERO;

    for cz in 0..n {
        for cx in 0..n {
            let t0 = std::time::Instant::now();
            let chunk = generate_chunk(seed, cx as i32 - r, cz as i32 - r);
            gen_time += t0.elapsed();
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
    println!(
        "  generation: {:.2}s over {} chunks ({:.2}ms/chunk)",
        gen_time.as_secs_f64(),
        n * n,
        gen_time.as_secs_f64() * 1000.0 / (n * n) as f64
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
                    block_color(Block::OakLog.id())
                } else if leaf {
                    block_color(Block::OakLeaves.id())
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

/// Full-depth vertical cross-section at world Z = `slice_z`, built from the
/// cubic per-section generator so caves below y = 0, cave-biome wall lining, and
/// deep ores are visible (the whole-column `side` preview stops at y = 0).
fn render_deep(seed: u32, out: &str, slice_z: i32, zoom: usize, center_x: i32) {
    use petramond::tooling::worldgen::{
        ChunkGenerator, Section, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
        WORLD_MIN_Y,
    };

    let zoom = zoom.max(1);
    let cells_x = 512usize;
    let cells_y = (256 - WORLD_MIN_Y) as usize;
    let x0 = center_x - cells_x as i32 / 2;
    let w = cells_x * zoom;
    let h = cells_y * zoom;
    let mut buf = vec![0u8; w * h * 3];

    let generator = ChunkGenerator::new(seed);
    let cz = slice_z.div_euclid(CHUNK_SZ as i32);
    let lz = slice_z.rem_euclid(CHUNK_SZ as i32) as usize;

    let mut cur_cx = i32::MIN;
    let mut sections: Vec<Section> = Vec::new();
    let mut surf = [0i32; 16];

    for gx in 0..cells_x {
        let wx = x0 + gx as i32;
        let cx = wx.div_euclid(CHUNK_SX as i32);
        let lx = wx.rem_euclid(CHUNK_SX as i32) as usize;
        if cx != cur_cx {
            let col = generator.generate_column_gen(cx, cz);
            for x in 0..16 {
                surf[x] = col.surface_y(x, lz);
            }
            sections = (SECTION_MIN_CY..=SECTION_MAX_CY)
                .map(|cy| generator.generate_section(SectionPos::new(cx, cy, cz), &col))
                .collect();
            cur_cx = cx;
        }
        for gy in 0..cells_y {
            let wy = 255 - gy as i32; // row 0 = world top
            let si = (wy.div_euclid(SECTION_SIZE as i32) - SECTION_MIN_CY) as usize;
            let ly = wy.rem_euclid(SECTION_SIZE as i32) as usize;
            let b = sections[si].block_raw(lx, ly, lz);
            let col = if b == 0 {
                if wy > surf[lx].max(63) {
                    [150, 190, 232] // open sky
                } else {
                    [16, 16, 20] // underground cave air
                }
            } else {
                block_color(b)
            };
            for dy in 0..zoom {
                for dx in 0..zoom {
                    let px = (gy * zoom + dy) * w + (gx * zoom + dx);
                    buf[px * 3..px * 3 + 3].copy_from_slice(&col);
                }
            }
        }
    }
    save(out, &buf, w, h);
    println!("wrote {out} ({w}x{h}, seed {seed:#x}, deep z={slice_z} zoom={zoom} cx={center_x})");
}

/// Horizontal cave-network map for the SECTION containing world Y = `slice_y`:
/// one pixel per column over a 512×512 window centred on the origin, marked
/// cave-air (black) if ANY of the section's 16 layers is carved there, marble
/// (light) if any layer is lined, else plain rock. The top-down projection that
/// makes tunnel topology (branch junctions, caverns, crawl-maze patches)
/// legible — a single-block slice only shows slivers of the mostly-horizontal
/// tunnels.
fn render_caveplan(seed: u32, out: &str, slice_y: i32, zoom: usize) {
    use petramond::tooling::worldgen::{ChunkGenerator, SectionPos, SECTION_SIZE};

    let zoom = zoom.max(1);
    let cells = 512usize;
    let half = cells as i32 / 2;
    let w = cells * zoom;
    let mut buf = vec![0u8; w * w * 3];

    let generator = ChunkGenerator::new(seed);
    let cy = slice_y.div_euclid(SECTION_SIZE as i32);
    let marble = Block::Marble.id();
    let n_chunks = (cells / 16) as i32;

    for gcz in 0..n_chunks {
        for gcx in 0..n_chunks {
            let (ccx, ccz) = (gcx - n_chunks / 2, gcz - n_chunks / 2);
            let col = generator.generate_column_gen(ccx, ccz);
            let section = generator.generate_section(SectionPos::new(ccx, cy, ccz), &col);
            for z in 0..16usize {
                for x in 0..16usize {
                    let wx = ccx * 16 + x as i32;
                    let wz = ccz * 16 + z as i32;
                    let (mut any_air, mut any_marble, mut top) = (false, false, 0u8);
                    for ly in 0..SECTION_SIZE {
                        let wy = cy * SECTION_SIZE as i32 + ly as i32;
                        if wy > col.surface_y(x, z) {
                            break;
                        }
                        let b = section.block_raw(x, ly, z);
                        any_air |= b == 0;
                        any_marble |= b == marble;
                        if b != 0 {
                            top = b;
                        }
                    }
                    let color = if any_air {
                        [10, 10, 12]
                    } else if any_marble {
                        [200, 200, 202]
                    } else {
                        block_color(top)
                    };
                    let (px, pz) = ((wx + half) as usize, (wz + half) as usize);
                    for dy in 0..zoom {
                        for dx in 0..zoom {
                            let p = (pz * zoom + dy) * w + px * zoom + dx;
                            buf[p * 3..p * 3 + 3].copy_from_slice(&color);
                        }
                    }
                }
            }
        }
    }
    save(out, &buf, w, w);
    println!("wrote {out} ({w}x{w}, seed {seed:#x}, caveplan section of y={slice_y})");
}

/// Cave + ore census over a chunk region, from the cubic per-section generator:
/// per-depth-band carved-air share, marble share, per-ore block counts, and the
/// surface entrance-mouth rate. The tuning instrument for the cave update.
fn cave_stats(seed: u32) {
    use petramond::tooling::worldgen::{
        ChunkGenerator, SectionPos, SECTION_MIN_CY, SECTION_SIZE, WORLD_MIN_Y,
    };
    use std::collections::HashMap;

    const R: i32 = 6; // 13x13 chunks
    const BAND: i32 = 32;
    let generator = ChunkGenerator::new(seed);

    // Per 32-block band: (sub-surface cells, carved air, marble).
    let bands = ((256 - WORLD_MIN_Y) / BAND) as usize;
    let mut band_cells = vec![0u64; bands];
    let mut band_air = vec![0u64; bands];
    let mut band_marble = vec![0u64; bands];
    let mut ore_counts: HashMap<u8, u64> = HashMap::new();
    let (mut mouth_columns, mut total_columns) = (0u64, 0u64);
    let mut chunks = 0u64;

    for cz in -R..=R {
        for cx in -R..=R {
            chunks += 1;
            let col = generator.generate_column_gen(cx, cz);
            let mut surf = [[0i32; 16]; 16];
            for z in 0..16 {
                for x in 0..16 {
                    surf[z][x] = col.surface_y(x, z);
                    total_columns += 1;
                    if col.heightmap_surface_y(x, z) < surf[z][x] {
                        mouth_columns += 1;
                    }
                }
            }
            let (_, surf_max) = col.surf_range();
            let top_cy = surf_max.div_euclid(SECTION_SIZE as i32);
            for cy in SECTION_MIN_CY..=top_cy {
                let section = generator.generate_section(SectionPos::new(cx, cy, cz), &col);
                let oy = cy * SECTION_SIZE as i32;
                for ly in 0..SECTION_SIZE {
                    let wy = oy + ly as i32;
                    let band = ((wy - WORLD_MIN_Y) / BAND) as usize;
                    for z in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            if wy > surf[z][x] {
                                continue;
                            }
                            let b = section.block_raw(x, ly, z);
                            band_cells[band] += 1;
                            if b == Block::Air.id() {
                                band_air[band] += 1;
                            } else if b == Block::Marble.id() {
                                band_marble[band] += 1;
                            } else if matches!(
                                Block::from_id(b),
                                Block::CoalOre
                                    | Block::IronOre
                                    | Block::CopperOre
                                    | Block::GoldOre
                                    | Block::DiamondOre
                            ) {
                                *ore_counts.entry(b).or_default() += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    println!("cave stats, seed {seed:#x}, {chunks} chunks:");
    println!("  y band        cells      cave-air  marble");
    for band in (0..bands).rev() {
        if band_cells[band] == 0 {
            continue;
        }
        let y0 = WORLD_MIN_Y + band as i32 * BAND;
        println!(
            "  [{:>4},{:>4})  {:>10}  {:>7.3}%  {:>6.3}%",
            y0,
            y0 + BAND,
            band_cells[band],
            band_air[band] as f64 / band_cells[band] as f64 * 100.0,
            band_marble[band] as f64 / band_cells[band] as f64 * 100.0,
        );
    }
    println!(
        "  entrance-mouth columns: {mouth_columns}/{total_columns} ({:.2}%)",
        mouth_columns as f64 / total_columns as f64 * 100.0
    );
    let mut ores: Vec<_> = ore_counts.into_iter().collect();
    ores.sort();
    for (b, n) in ores {
        println!(
            "  {:?}: {n} total, {:.2}/chunk",
            Block::from_id(b),
            n as f64 / chunks as f64
        );
    }
}

/// Audit overhangs + floating debris across a region (computed by
/// `worldgen::audit::audit`); prints its [`DebrisAudit`] in the previewer format.
fn audit(seed: u32) {
    use petramond::tooling::worldgen::audit;
    let a = audit::audit(seed);
    println!(
        "seed {seed:#x}: overhang-ceilings {}  floating-debris {}  deepest-ocean-floor y{}  tallest y{} @ (x{},z{})",
        a.overhang_ceilings,
        a.floating_debris,
        a.deepest_ocean_floor,
        a.tallest_y,
        a.tallest_xz.0,
        a.tallest_xz.1
    );
    println!("  tallest skin: {}", a.tallest_skin);
    println!(
        "  overhangiest column: {} ceilings @ (x{},z{})",
        a.overhangiest, a.overhangiest_xz.0, a.overhangiest_xz.1
    );
    let line: Vec<String> = a
        .biomes
        .iter()
        .map(|s| format!("{} {:.1}%", s.name, s.percent))
        .collect();
    println!("  biomes: {}", line.join(", "));
}

/// True 3-D detached-debris census (computed by `worldgen::audit::flood_audit`);
/// prints its [`FloodAudit`] in the previewer format.
fn flood_audit(seed: u32) {
    use petramond::tooling::worldgen::audit;
    let f = audit::flood_audit(seed);
    let (w, _, hgt) = f.region;
    println!(
        "seed {seed:#x}: solids {}  detached-debris {} ({:.1} ppm of solid terrain), region {w}x{w}x{hgt}",
        f.solids,
        f.detached_debris,
        f.ppm()
    );
}

/// Lowland-relief diagnostic (computed by `worldgen::audit::relief_audit`);
/// prints its [`ReliefStats`] in the previewer format.
fn relief_audit(seed: u32) {
    use petramond::tooling::worldgen::audit::{self, RELIEF_HIST_LABELS};
    let r = audit::relief_audit(seed);
    if r.land.count == 0 {
        println!("seed {seed:#x}: no land-biome columns in window");
        return;
    }
    let l = &r.land;
    println!(
        "seed {seed:#x}: land-biome relief over {} cols ({}x{} blocks)",
        l.count, r.window_blocks, r.window_blocks
    );
    println!(
        "  surf-Y: min {} p10 {} p50 {} p90 {} max {}  mean {:.2}  STDEV {:.3}",
        l.min, l.p10, l.p50, l.p90, l.max, l.mean, l.stdev
    );
    println!(
        "  at-waterline: {:.2}% ({}/{})   <- dry land at sea level",
        r.at_waterline_pct, r.at_waterline, l.count
    );
    println!(
        "  flooded-land: {:.3}% ({})   <- pond-maze metric (surf < sea level)",
        r.flooded_pct, r.flooded
    );
    let bars: Vec<String> = RELIEF_HIST_LABELS
        .iter()
        .zip(r.hist_pct.iter())
        .map(|(label, &c)| format!("{label}:{c:.1}%"))
        .collect();
    println!("  hist  {}", bars.join("  "));
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

/// Walkability / spikiness metric (computed by `worldgen::audit::roughness`);
/// prints its [`RoughnessStats`] in the previewer format.
fn roughness(seed: u32) {
    use petramond::tooling::worldgen::audit;
    let Some(s) = audit::roughness(seed) else {
        println!("seed {seed:#x}: no mountain-like biome columns in region");
        return;
    };
    println!(
        "seed {seed:#x}: mtn-cols {}  mean-max-step {:.2}  pillar {:.1}%  walkable {:.1}%",
        s.mountain_cols, s.mean_max_step, s.pillar_pct, s.walkable_pct
    );
    let h = s.max_step_hist_pct;
    println!(
        "  max-step hist (0,1,2,3,4,5+): {:.0}% {:.0}% {:.0}% {:.0}% {:.0}% {:.0}%",
        h[0], h[1], h[2], h[3], h[4], h[5]
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
        for (sx, yb) in ybuf.iter_mut().enumerate() {
            let t = sx as f32 / (w - 1) as f32;
            let wx = cam_x as f32 + (-half + 2.0 * half * t);
            let (b, hy) = surf(wx.round() as i32, wz, &mut cache);
            let sy = horizon + (cam_y as f32 - hy as f32) * scale / df;
            let sy = sy.clamp(0.0, h as f32);
            if sy < *yb {
                // simple depth + slope shade
                let shade = (1.15 - df / (max_d as f32) * 0.85).clamp(0.25, 1.0);
                let mut col = block_color(b);
                for c in &mut col {
                    *c = (*c as f32 * shade) as u8;
                }
                let y0 = sy.max(0.0) as usize;
                let y1 = *yb as usize;
                for y in y0..y1 {
                    let p = (y * w + sx) * 3;
                    buf[p..p + 3].copy_from_slice(&col);
                }
                *yb = sy;
            }
        }
    }
    save(out, &buf, w, h);
    println!("wrote {out} ({w}x{h}, seed {seed:#x}, view cam=({cam_x},{cam_y},{cam_z}))");
}

/// Km-scale overview: biome colour shaded by the base-height relief, sampled
/// from the climate graph on a stride grid (no chunk generation, so an
/// 8192-block window renders in seconds). Mountain belts, valley networks and
/// coast shapes are only visible at this scale.
fn render_macro(seed: u32, out: &str, stride: i32) {
    let side = 512usize;
    let map = macro_surface_map(seed, side, stride.max(1));
    let mut buf = vec![0u8; side * side * 3];
    for i in 0..side * side {
        let mut col = biome_color(map.biomes[i]);
        let shade = (0.55 + 0.010 * (map.heights[i] - 58.0) as f32).clamp(0.40, 1.45);
        for c in &mut col {
            *c = (*c as f32 * shade).clamp(0.0, 255.0) as u8;
        }
        buf[i * 3..i * 3 + 3].copy_from_slice(&col);
    }
    save(out, &buf, side, side);
    println!(
        "wrote {out} ({side}x{side}, seed {seed:#x}, macro stride {stride} = {} blocks/edge)",
        side as i32 * stride
    );
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
        // macro <stride>: km-scale biome+relief overview straight from the
        // climate graph (no chunk gen) — 512 samples/edge, default stride 16
        // blocks ⇒ an 8192-block window. For world-structure verification.
        "macro" => render_macro(seed, &out, arg.unwrap_or(16)),
        "side" => render_side(seed, &out, arg.unwrap_or(0), zoom, center_x, proj),
        // deep <z>: full-depth (-64..255) cross-section from the cubic
        // per-section generator — caves below y=0, marble lining, deep ores.
        "deep" => render_deep(seed, &out, arg.unwrap_or(0), zoom, center_x),
        // caveplan <y>: horizontal cave-network slice at world Y (tunnel
        // topology: branches, junctions, caverns).
        "caveplan" => render_caveplan(seed, &out, arg.unwrap_or(-16), zoom),
        // cavestats: cave/ore census (carved share per depth band, marble share,
        // ore per chunk, entrance rate).
        "cavestats" => cave_stats(seed),
        "shade" => render_shaded(
            seed,
            &out,
            center_x,
            arg.unwrap_or(0),
            zoom.max(1) as i32,
            proj,
        ),
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
        "relief" => relief_audit(seed),
        _ => render_topdown(seed, &out, false),
    }
}
