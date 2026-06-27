// Build-time texture atlas composer.
//
// Reads block textures from assets/textures/*.png, tiles them into a single
// PNG atlas, emits Rust constants for tile indices and atlas dimensions.
// Each source texture is resampled to fixed 16x16 (one block-face tile).
// Static entries that reference animated vertical strips use the first frame.

use std::env;
use std::fs;
use std::path::PathBuf;

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
    // Crafting update (Survival 0.1): crafted blocks + flat item sprites. Appended
    // last so every preceding tile id stays stable (tile ids aren't persisted, but
    // the convention keeps diffs minimal). Block tiles render as cubes; the item
    // tiles below are flat sprites referenced by `ItemType::item_sprite`.
    ("cobblestone", "cobblestone.png"),
    ("oak_planks", "oak_planks.png"),
    ("spruce_planks", "spruce_planks.png"),
    ("birch_planks", "birch_planks.png"),
    ("jungle_planks", "jungle_planks.png"),
    ("acacia_planks", "acacia_planks.png"),
    ("dark_oak_planks", "dark_oak_planks.png"),
    ("cherry_planks", "cherry_planks.png"),
    ("mangrove_planks", "mangrove_planks.png"),
    ("crafting_table_top", "crafting_table_top.png"),
    ("crafting_table_front", "crafting_table_front.png"),
    // Flat item sprites (tools + drops): drawn as billboards in slots / in-hand.
    ("stick", "stick.png"),
    ("wooden_pickaxe", "wooden_pickaxe.png"),
    ("stone_pickaxe", "stone_pickaxe.png"),
    ("raw_iron", "raw_iron.png"),
    ("raw_copper", "raw_copper.png"),
    ("coal", "coal.png"),
    // Furnace update (Survival 0.1): furnace block faces + the "on" front shown
    // while burning, and the two smelted ingots. Appended last so every preceding
    // tile id stays stable.
    ("furnace_top", "furnace_top.png"),
    ("furnace_front", "furnace_front.png"),
    ("furnace_front_on", "furnace_front_on.png"),
    ("furnace_side", "furnace_side.png"),
    ("iron_ingot", "iron_ingot.png"),
    ("copper_ingot", "copper_ingot.png"),
    // Chest update (Survival 0.1): chest face tiles, sliced from the vanilla
    // single-chest entity texture (entity/chest/normal.png) into per-face tiles.
    // The placed chest is drawn as a custom inset body + hinged lid model (see
    // render::chest_model); these tiles texture that model and the inventory icon.
    // Appended last so every preceding tile id stays stable.
    ("chest_top", "chest_top.png"),
    ("chest_front", "chest_front.png"),
    ("chest_side", "chest_side.png"),
    ("chest_lid_front", "chest_lid_front.png"),
    ("chest_lid_side", "chest_lid_side.png"),
    ("chest_inside", "chest_inside.png"),
    ("chest_latch", "chest_latch.png"),
    // Torch update (Survival 0.1): the full torch sprite (item icon + held), plus
    // two cropped tiles for the in-world 3D model — the center-strip body shown on
    // the four thin side faces, and the flame cap on the top face. Cropped because
    // the chunk shader maps a WHOLE tile per face, so each thin torch face needs a
    // tile that is already just its slice of the sprite. Appended last to keep all
    // preceding tile ids stable.
    ("torch", "torch.png"),
    ("torch_side", "torch_side.png"),
    ("torch_top", "torch_top.png"),
    // Tools + ores update: diamond/lapis/raw-gold/gold-ingot drops and the iron/
    // diamond pickaxes + the four axe tiers. Flat item sprites (billboards in
    // slots / in-hand), referenced by `ItemType::item_sprite`. Appended last so
    // every preceding tile id stays stable.
    ("diamond", "diamond.png"),
    ("lapis_lazuli", "lapis_lazuli.png"),
    ("raw_gold", "raw_gold.png"),
    ("gold_ingot", "gold_ingot.png"),
    ("wooden_axe", "wooden_axe.png"),
    ("stone_axe", "stone_axe.png"),
    ("iron_axe", "iron_axe.png"),
    ("diamond_axe", "diamond_axe.png"),
    ("iron_pickaxe", "iron_pickaxe.png"),
    ("diamond_pickaxe", "diamond_pickaxe.png"),
    // Shovels: the four shovel tiers (dirt/sand tool). Flat item sprites
    // referenced by `ItemType::item_sprite`. Appended last so every preceding
    // tile id stays stable.
    ("wooden_shovel", "wooden_shovel.png"),
    ("stone_shovel", "stone_shovel.png"),
    ("iron_shovel", "iron_shovel.png"),
    ("diamond_shovel", "diamond_shovel.png"),
    // Saplings update: the cross-plant saplings dropped by leaves and grown into
    // trees (one per tree species that has a feature). Cutout sprites rendered as
    // `RenderShape::Cross`, like the flowers. Appended last so every preceding
    // tile id stays stable.
    ("oak_sapling", "oak_sapling.png"),
    ("spruce_sapling", "spruce_sapling.png"),
    ("birch_sapling", "birch_sapling.png"),
    ("jungle_sapling", "jungle_sapling.png"),
    ("acacia_sapling", "acacia_sapling.png"),
    ("dark_oak_sapling", "dark_oak_sapling.png"),
    ("cherry_sapling", "cherry_sapling.png"),
    // Doors update: per-species wooden doors. Each door is a 2-tall thin block
    // drawn as a dynamic hinged model (see render::door_model), so the `_top` /
    // `_bottom` tiles texture the upper / lower halves' front+back faces (and a
    // 3px slice of each textures the thin side edges), while `_door_item` is the
    // flat inventory sprite. Appended last so every preceding tile id stays stable.
    ("oak_door_top", "oak_door_top.png"),
    ("oak_door_bottom", "oak_door_bottom.png"),
    ("spruce_door_top", "spruce_door_top.png"),
    ("spruce_door_bottom", "spruce_door_bottom.png"),
    ("birch_door_top", "birch_door_top.png"),
    ("birch_door_bottom", "birch_door_bottom.png"),
    ("jungle_door_top", "jungle_door_top.png"),
    ("jungle_door_bottom", "jungle_door_bottom.png"),
    ("acacia_door_top", "acacia_door_top.png"),
    ("acacia_door_bottom", "acacia_door_bottom.png"),
    ("dark_oak_door_top", "dark_oak_door_top.png"),
    ("dark_oak_door_bottom", "dark_oak_door_bottom.png"),
    ("cherry_door_top", "cherry_door_top.png"),
    ("cherry_door_bottom", "cherry_door_bottom.png"),
    ("mangrove_door_top", "mangrove_door_top.png"),
    ("mangrove_door_bottom", "mangrove_door_bottom.png"),
    ("oak_door_item", "oak_door_item.png"),
    ("spruce_door_item", "spruce_door_item.png"),
    ("birch_door_item", "birch_door_item.png"),
    ("jungle_door_item", "jungle_door_item.png"),
    ("acacia_door_item", "acacia_door_item.png"),
    ("dark_oak_door_item", "dark_oak_door_item.png"),
    ("cherry_door_item", "cherry_door_item.png"),
    ("mangrove_door_item", "mangrove_door_item.png"),
];

/// Animated flipbook tiles (name, file). The source PNG is a vertical strip of
/// square cells; each is resampled to 16x16 and laid out as CONSECUTIVE atlas
/// tiles starting at the base tile. The base `Tile` reports its derived frame
/// count via `Tile::anim_frames`; the block shader advances `base + frame` over
/// time (see `block.wgsl`). Appended after the static tiles so all existing tile
/// ids stay stable.
const ANIM_TILES: &[(&str, &str)] = &[
    ("water_still", "water_still.png"),
    ("water_flow", "water_flow.png"),
];

fn main() {
    println!("cargo:rerun-if-changed=assets/textures");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let in_dir = manifest_dir.join("assets/textures");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let atlas_png = out_dir.join("atlas.png");
    let atlas_rs = out_dir.join("atlas_data.rs");

    // Flatten static tiles (one frame each) and animated tiles (a vertical
    // flipbook expanded into consecutive 16x16 frames) into one ordered list of
    // (name, 16x16 RGBA) cells. `anim_meta` records each animated base's frame
    // count for the generated `Tile::anim_frames`.
    let load_16 = |file: &str| -> image::RgbaImage {
        let path = in_dir.join(file);
        if !path.exists() {
            panic!("missing texture: {}", path.display());
        }
        let img = image::open(&path)
            .unwrap_or_else(|e| panic!("failed to load {}: {}", path.display(), e))
            .to_rgba8();
        let (w, h) = (img.width(), img.height());
        let frame = if w > 0 && h > w && h % w == 0 {
            image::imageops::crop_imm(&img, 0, 0, w, w).to_image()
        } else {
            img
        };
        image::imageops::resize(&frame, TILE, TILE, image::imageops::FilterType::Nearest)
    };

    let mut cells: Vec<(String, image::RgbaImage)> = Vec::new();
    for (name, file) in TILES {
        cells.push((name.to_string(), load_16(file)));
    }
    let mut anim_meta: Vec<(String, u32)> = Vec::new();
    for (name, file) in ANIM_TILES {
        let path = in_dir.join(file);
        if !path.exists() {
            panic!("missing texture: {}", path.display());
        }
        let strip = image::open(&path)
            .unwrap_or_else(|e| panic!("failed to load {}: {}", path.display(), e))
            .to_rgba8();
        let (sw, sh) = (strip.width(), strip.height());
        if sw == 0 || sh == 0 || sh % sw != 0 {
            panic!(
                "animated texture {} must be a vertical strip of square frames, got {}x{}",
                path.display(),
                sw,
                sh
            );
        }
        let frames = sh / sw;
        for i in 0..frames {
            let fh = sw;
            let frame = image::imageops::crop_imm(&strip, 0, i * fh, sw, fh).to_image();
            let tile =
                image::imageops::resize(&frame, TILE, TILE, image::imageops::FilterType::Nearest);
            let cell_name = if i == 0 {
                name.to_string()
            } else {
                format!("{name}_{i}")
            };
            cells.push((cell_name, tile));
        }
        anim_meta.push((to_camel(name), frames));
    }

    let count = cells.len() as u32;
    // Square-ish atlas. cols = ceil(sqrt(n)); rows = ceil(n/cols).
    let cols = (count as f32).sqrt().ceil() as u32;
    let rows = count.div_ceil(cols);
    let atlas_w = cols * TILE;
    let atlas_h = rows * TILE;

    // RGBA buffer.
    let mut buf = vec![0u8; (atlas_w * atlas_h * 4) as usize];

    let mut entries: Vec<(String, u32, u32)> = Vec::new();
    for (i, (name, resized)) in cells.iter().enumerate() {
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
    src.push_str("    /// Flipbook frame count for an animated tile (0 = static). The frames\n");
    src.push_str("    /// occupy this tile's index and the next `n-1` consecutive tiles, so\n");
    src.push_str("    /// the shader samples `base + frame` to animate.\n");
    src.push_str("    pub fn anim_frames(self) -> u32 {\n        match self {\n");
    for (camel, frames) in &anim_meta {
        src.push_str(&format!("            Tile::{} => {},\n", camel, frames));
    }
    src.push_str("            _ => 0,\n        }\n    }\n");
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
}
