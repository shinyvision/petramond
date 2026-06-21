// Build-time texture atlas composer.
//
// Reads block textures from assets/textures/*.png, tiles them into a single
// PNG atlas, emits Rust constants for tile indices and atlas dimensions.
// Each source texture is resampled to fixed 16x16 (one block-face tile).

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const TILE: u32 = 16;

/// snake_case -> CamelCase (e.g. "grass_top" -> "GrassTop").
fn to_camel(s: &str) -> String {
    s.split('_')
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

// (name, file). Order defines tile index in atlas (stable for shader uv math).
const TILES: &[(&str, &str)] = &[
    ("grass_top", "grass_block_top.png"),
    ("grass_side", "grass_block_side.png"),
    ("grass_snow", "grass_block_snow.png"),
    ("dirt", "dirt.png"),
    ("stone", "stone.png"),
    ("sand", "sand.png"),
    ("water", "water_still.png"),
    ("oak_log_side", "oak_log.png"),
    ("oak_log_top", "oak_log_top.png"),
    ("oak_leaves", "oak_leaves.png"),
    // Appended (keeps tiles 0..9 stable): the snowy-block top, and the
    // grayscale grass side overlay that is biome-tinted + composited over dirt
    // so the grass on a block's side matches its tinted top.
    ("snow", "snow.png"),
    ("grass_side_overlay", "grass_block_side_overlay.png"),
    ("spruce_log_top", "spruce_log_top.png"),
    ("spruce_log_side", "spruce_log.png"),
    ("birch_log_top", "birch_log_top.png"),
    ("birch_log_side", "birch_log.png"),
    ("jungle_log_top", "jungle_log_top.png"),
    ("jungle_log_side", "jungle_log.png"),
    ("acacia_log_top", "acacia_log_top.png"),
    ("acacia_log_side", "acacia_log.png"),
    ("dark_oak_log_top", "dark_oak_log_top.png"),
    ("dark_oak_log_side", "dark_oak_log.png"),
    ("cherry_log_top", "cherry_log_top.png"),
    ("cherry_log_side", "cherry_log.png"),
    ("mangrove_log_top", "mangrove_log_top.png"),
    ("mangrove_log_side", "mangrove_log.png"),
    ("spruce_leaves", "spruce_leaves.png"),
    ("birch_leaves", "birch_leaves.png"),
    ("jungle_leaves", "jungle_leaves.png"),
    ("acacia_leaves", "acacia_leaves.png"),
    ("dark_oak_leaves", "dark_oak_leaves.png"),
    ("mangrove_leaves", "mangrove_leaves.png"),
    ("cherry_leaves", "cherry_leaves.png"),
    ("azalea_leaves", "azalea_leaves.png"),
    ("red_sand", "red_sand.png"),
    ("sandstone_top", "sandstone_top.png"),
    ("sandstone_bottom", "sandstone_bottom.png"),
    ("sandstone_side", "sandstone.png"),
    ("red_sandstone_top", "red_sandstone_top.png"),
    ("red_sandstone_bottom", "red_sandstone_bottom.png"),
    ("red_sandstone_side", "red_sandstone.png"),
    ("terracotta", "terracotta.png"),
    ("white_terracotta", "white_terracotta.png"),
    ("orange_terracotta", "orange_terracotta.png"),
    ("yellow_terracotta", "yellow_terracotta.png"),
    ("brown_terracotta", "brown_terracotta.png"),
    ("red_terracotta", "red_terracotta.png"),
    ("light_gray_terracotta", "light_gray_terracotta.png"),
    ("podzol_top", "podzol_top.png"),
    ("podzol_side", "podzol_side.png"),
    ("mycelium_top", "mycelium_top.png"),
    ("mycelium_side", "mycelium_side.png"),
    ("coarse_dirt", "coarse_dirt.png"),
    ("gravel", "gravel.png"),
    ("clay", "clay.png"),
    ("mud", "mud.png"),
    ("moss_block", "moss_block.png"),
    ("packed_ice", "packed_ice.png"),
    ("ice", "ice.png"),
    ("calcite", "calcite.png"),
    ("granite", "granite.png"),
    ("diorite", "diorite.png"),
    ("andesite", "andesite.png"),
    ("tuff", "tuff.png"),
    ("coal_ore", "coal_ore.png"),
    ("iron_ore", "iron_ore.png"),
    ("copper_ore", "copper_ore.png"),
    ("gold_ore", "gold_ore.png"),
    ("redstone_ore", "redstone_ore.png"),
    ("lapis_ore", "lapis_ore.png"),
    ("diamond_ore", "diamond_ore.png"),
    ("emerald_ore", "emerald_ore.png"),
    ("pumpkin_top", "pumpkin_top.png"),
    ("pumpkin_side", "pumpkin_side.png"),
    ("melon_top", "melon_top.png"),
    ("melon_side", "melon_side.png"),
    ("cactus_top", "cactus_top.png"),
    ("cactus_bottom", "cactus_bottom.png"),
    ("cactus_side", "cactus_side.png"),
    ("short_grass", "short_grass.png"),
    ("fern", "fern.png"),
    ("dandelion", "dandelion.png"),
    ("poppy", "poppy.png"),
    ("cornflower", "cornflower.png"),
    ("allium", "allium.png"),
    ("azure_bluet", "azure_bluet.png"),
    ("oxeye_daisy", "oxeye_daisy.png"),
    ("red_tulip", "red_tulip.png"),
    ("dead_bush", "dead_bush.png"),
    ("brown_mushroom", "brown_mushroom.png"),
    ("red_mushroom", "red_mushroom.png"),
    // Block-breaking overlay stages (Survival 0.1). Appended last so all
    // preceding tile ids stay stable (chunk/save compatibility; tests enforce).
    // 16x16 grayscale+alpha cracks; sampled by the break-overlay pass over the
    // mined face. Generates Tile::DestroyStage0 .. Tile::DestroyStage9.
    ("destroy_stage_0", "destroy_stage_0.png"),
    ("destroy_stage_1", "destroy_stage_1.png"),
    ("destroy_stage_2", "destroy_stage_2.png"),
    ("destroy_stage_3", "destroy_stage_3.png"),
    ("destroy_stage_4", "destroy_stage_4.png"),
    ("destroy_stage_5", "destroy_stage_5.png"),
    ("destroy_stage_6", "destroy_stage_6.png"),
    ("destroy_stage_7", "destroy_stage_7.png"),
    ("destroy_stage_8", "destroy_stage_8.png"),
    ("destroy_stage_9", "destroy_stage_9.png"),
];

fn main() {
    println!("cargo:rerun-if-changed=assets/textures");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let in_dir = manifest_dir.join("assets/textures");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let atlas_png = out_dir.join("atlas.png");
    let atlas_rs = out_dir.join("atlas_data.rs");

    let count = TILES.len() as u32;
    // Square-ish atlas. cols = ceil(sqrt(n)); rows = ceil(n/cols).
    let cols = (count as f32).sqrt().ceil() as u32;
    let rows = count.div_ceil(cols);
    let atlas_w = cols * TILE;
    let atlas_h = rows * TILE;

    // RGBA buffer.
    let mut buf = vec![0u8; (atlas_w * atlas_h * 4) as usize];

    let mut entries: Vec<(String, u32, u32)> = Vec::new();
    for (i, (name, file)) in TILES.iter().enumerate() {
        let path = in_dir.join(file);
        if !path.exists() {
            panic!("missing texture: {}", path.display());
        }
        // Use `image` crate to load + resize to 16x16 RGBA.
        let img = image::open(&path)
            .unwrap_or_else(|e| panic!("failed to load {}: {}", path.display(), e))
            .to_rgba8();
        let resized =
            image::imageops::resize(&img, TILE, TILE, image::imageops::FilterType::Nearest);
        let tile_col = i as u32 % cols;
        let tile_row = i as u32 / cols;
        let base_x = tile_col * TILE;
        let base_y = tile_row * TILE;
        for y in 0..TILE {
            for x in 0..TILE {
                let px = resized.get_pixel(x, y);
                let dst = ((base_y + y) * atlas_w + base_x + x) as usize * 4;
                buf[dst] = px.0[0];
                buf[dst + 1] = px.0[1];
                buf[dst + 2] = px.0[2];
                buf[dst + 3] = px.0[3];
            }
        }
        entries.push((name.to_string(), tile_col, tile_row));
    }

    // Write atlas PNG (used by runtime to create GPU texture).
    image::save_buffer(&atlas_png, &buf, atlas_w, atlas_h, image::ColorType::Rgba8)
        .expect("write atlas png");

    // Write Rust source with tile indices + uv layout.
    let mut src = String::new();
    src.push_str("// AUTO-GENERATED by build.rs. Do not edit.\n");
    src.push_str(&format!("pub const ATLAS_W: u32 = {};\n", atlas_w));
    src.push_str(&format!("pub const ATLAS_H: u32 = {};\n", atlas_h));
    src.push_str(&format!("pub const ATLAS_COLS: u32 = {};\n", cols));
    src.push_str(&format!("pub const ATLAS_ROWS: u32 = {};\n", rows));
    src.push_str("pub const TILE: u32 = 16;\n\n");
    src.push_str("#[derive(Copy, Clone, Debug, PartialEq, Eq)]\n");
    src.push_str("pub enum Tile {\n");
    for (name, _, _) in &entries {
        let ident = to_camel(name);
        src.push_str(&format!("    {},\n", ident));
    }
    src.push_str("}\n\n");
    src.push_str("impl Tile {\n");
    src.push_str("    pub const ALL: &'static [Tile] = &[\n");
    for (name, _, _) in &entries {
        let ident = to_camel(name);
        src.push_str(&format!("        Tile::{},\n", ident));
    }
    src.push_str("    ];\n");
    src.push_str("    pub fn index(self) -> usize {\n        self as usize\n    }\n");
    src.push_str("    /// (col, row) in atlas tile grid.\n");
    src.push_str("    pub fn grid(self) -> (u32, u32) {\n        #[allow(unused)] let _ = (); match self {\n");
    for (name, col, row) in &entries {
        let ident = to_camel(name);
        src.push_str(&format!(
            "            Tile::{} => ({}, {}),\n",
            ident, col, row
        ));
    }
    src.push_str("        }\n    }\n");
    src.push_str("}\n\n");
    src.push_str(&format!(
        "pub const TILE_COUNT: usize = {};\n",
        entries.len()
    ));

    fs::write(&atlas_rs, src).expect("write atlas_data.rs");

    // Tell downstream where atlas png lives via envvar passed to tests/tests.
    println!(
        "cargo:rustc-env=LLAMACRAFT_ATLAS_PNG={}",
        atlas_png.display()
    );
    println!("cargo:rustc-env=LLAMACRAFT_ATLAS_DIR={}", out_dir.display());

    // Try to also copy atlas into web/ for inspection (best-effort).
    let _ = Command::new("cp")
        .arg(&atlas_png)
        .arg(manifest_dir.join("web/atlas.png"))
        .status();
}
