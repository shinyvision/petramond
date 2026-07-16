//! Runtime texture atlas: composed at startup from `assets/textures/` per the
//! `assets/textures/atlas.json` manifest, plus tile lookup.
//!
//! The manifest lists every tile by stable snake_case name — the identity block
//! and item data files reference — with its source PNG and per-tile render
//! columns (biome-tint class, animation, mip cutout-fill). Adding a texture is
//! adding a manifest row + a PNG: no rebuild. Each source texture is resampled
//! to a fixed 16×16 tile; an animated entry is a vertical strip of square
//! frames expanded into consecutive tiles so the block shader can advance
//! `base + frame` over time.
//!
//! Like the block/item tables, the atlas is load-bearing (meshing, render, and
//! data-file tile references all resolve through it), so loading validates the
//! manifest fully and panics with a precise message rather than misrendering.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

use serde::Deserialize;

/// Fixed tile edge length in texels. Every atlas cell is `TILE × TILE`.
pub const TILE: u32 = 16;

const TILE_SIZE: usize = TILE as usize;
const ALPHA_CUTOFF: u8 = 128;

/// One 16×16 cell of the atlas, identified by its load-time index. Stable
/// WITHIN a run (data files reference tiles by name; numeric ids are assigned
/// at load and never persisted).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tile(u16);

/// Which biome colour a tile tints with (see `mesh::tint` for the in-world
/// blend and `render::foliage_tint` for the fixed out-of-world colour).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileTint {
    Grass,
    Foliage,
    Water,
}

impl Tile {
    /// This tile's atlas index (also its texture-array layer).
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// This tile's index as the raw id the packed chunk vertex carries.
    #[inline]
    pub fn id(self) -> u16 {
        self.0
    }

    /// (col, row) in the atlas tile grid.
    #[inline]
    pub fn grid(self) -> (u32, u32) {
        let d = data();
        (self.0 as u32 % d.cols, self.0 as u32 / d.cols)
    }

    /// The tile's stable snake_case asset name (its manifest identity).
    #[inline]
    pub fn name(self) -> &'static str {
        data().names[self.index()]
    }

    /// Resolve a manifest name to its tile.
    pub fn from_name(name: &str) -> Option<Tile> {
        data().by_name.get(name).copied()
    }

    /// [`from_name`](Self::from_name) for a tile the caller knows must exist
    /// (engine code, tests); panics with the name if the manifest lost it.
    pub fn named(name: &str) -> Tile {
        Tile::from_name(name).unwrap_or_else(|| panic!("no tile named '{name}' in the atlas"))
    }

    /// Flipbook frame count for an animated tile (0 = static). The frames
    /// occupy this tile's index and the next `n-1` consecutive tiles, so the
    /// shader samples `base + frame` to animate.
    #[inline]
    pub fn anim_frames(self) -> u32 {
        data().anim_frames[self.index()]
    }

    /// This tile's IN-WORLD biome-tint class (what the chunk mesher applies),
    /// from its manifest row. `None` = untinted.
    #[inline]
    pub fn world_tint(self) -> Option<TileTint> {
        data().world_tint[self.index()]
    }

    /// This tile's OUT-OF-WORLD tint class — icons, held/dropped items, and
    /// break particles, which have no biome context. Defaults to
    /// [`world_tint`](Self::world_tint); a manifest row overrides it with
    /// `icon_tint` when the two classifications differ (azalea leaves
    /// green in icons but keep their baked colour in the world).
    #[inline]
    pub fn icon_tint(self) -> Option<TileTint> {
        let d = data();
        d.icon_tint[self.index()].or(d.world_tint[self.index()])
    }

    /// Number of tiles in the atlas.
    #[inline]
    pub fn count() -> usize {
        data().count
    }

    /// Every tile, in id order.
    pub fn all() -> impl Iterator<Item = Tile> {
        (0..Tile::count() as u16).map(Tile)
    }

    /// Representative untinted top-down cartography colour, derived once from
    /// the composed tile pixels. Callers apply the same biome tint as terrain.
    pub fn map_rgb(self) -> [u8; 3] {
        data().map_rgb[self.index()]
    }

    /// Lowest alpha across this tile's texels — the asset↔render-pass
    /// contract's input: an OPAQUE block row's tiles must be genuinely opaque
    /// (≥ 128), a TRANSLUCENT row's tiles must author alpha in the 0.25..0.5
    /// band (above the cutout passes' `a < 0.25` discard, below water's 0.5
    /// split in `fs_transparent`). Pinned by
    /// `block_tiles_match_their_render_pass_alpha_contract`.
    pub fn min_alpha(self) -> u8 {
        data().min_alpha[self.index()]
    }
}

/// Tiles the ENGINE itself references (shader uniforms, the custom chest model,
/// the grass-side compositing, the break-overlay stages) — resolved once at
/// atlas load. Content tiles flow through block/item data rows instead; a tile
/// belongs here only when engine CODE, not data, needs it.
pub struct EngineTiles {
    pub water_still: Tile,
    pub water_flow: Tile,
    /// The grass-block side compositing set: an untinted `dirt` base with the
    /// biome-tinted grayscale `grass_side_overlay` on top, applied wherever the
    /// mesher (or the out-of-world item renderer) meets `grass_side`.
    pub grass_side: Tile,
    pub grass_side_overlay: Tile,
    /// Untinted snowy grass side, swapped in for the compositing set wherever
    /// a snow-cover block sits directly on the grass (see the mesher).
    pub grass_snow: Tile,
    pub dirt: Tile,
    /// Furnace faces: the mesher swaps `furnace_front` / `furnace_front_on` by
    /// the block-entity's lit state and emits `furnace_side` around it.
    pub furnace_side: Tile,
    pub furnace_front: Tile,
    pub furnace_front_on: Tile,
    /// The custom inset chest model's face set (see `render::chest_model`).
    pub chest_top: Tile,
    pub chest_front: Tile,
    pub chest_side: Tile,
    pub chest_lid_front: Tile,
    pub chest_lid_side: Tile,
    pub chest_inside: Tile,
    pub chest_latch: Tile,
    /// Break-progress crack overlays, stage 0 (first crack) to 9 (shattering).
    pub destroy_stages: [Tile; 10],
}

/// The engine-referenced tiles, resolved once at atlas load.
#[inline]
pub fn engine() -> &'static EngineTiles {
    &data().engine
}

// ---------------------------------------------------------------------------------
// Manifest + composition
// ---------------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawManifest {
    tiles: Vec<RawTile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTile {
    name: String,
    file: String,
    /// A vertical strip of square frames, expanded into consecutive tiles.
    #[serde(default)]
    anim: bool,
    #[serde(default)]
    tint: Option<TileTint>,
    #[serde(default)]
    icon_tint: Option<TileTint>,
    /// Alpha-expand while downsampling mips, so distant cutout gaps fill with
    /// nearby colour instead of disappearing under the shader's alpha test.
    #[serde(default)]
    fill_cutout_mips: bool,
}

struct AtlasData {
    count: usize,
    cols: u32,
    rows: u32,
    names: Vec<&'static str>,
    by_name: HashMap<&'static str, Tile>,
    anim_frames: Vec<u32>,
    world_tint: Vec<Option<TileTint>>,
    icon_tint: Vec<Option<TileTint>>,
    fill_cutout_mips: Vec<bool>,
    map_rgb: Vec<[u8; 3]>,
    /// Lowest alpha across the tile's texels — the asset↔shader contract
    /// check: an OPAQUE block row's tiles must survive the block shader's
    /// cutout (`block.wgsl` discards `a < 0.5`), or the block renders as a
    /// hole (the invisible-ice bug, 2026-07-16).
    min_alpha: Vec<u8>,
    /// Composed base atlas, `cols*TILE × rows*TILE` RGBA.
    rgba: Vec<u8>,
    engine: EngineTiles,
}

static ATLAS: LazyLock<AtlasData> = LazyLock::new(|| {
    let layers = crate::assets::read_layers("textures/atlas.json");
    if layers.is_empty() {
        panic!(
            "textures/atlas.json not found (searched {:?}); the game cannot run without its texture atlas",
            crate::assets::candidate_paths("textures/atlas.json")
        );
    }
    for (_, path) in &layers {
        log::info!("atlas manifest layer: {}", path.display());
    }
    let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
    build(&texts).unwrap_or_else(|e| panic!("textures/atlas.json: {e}"))
});

#[inline]
fn data() -> &'static AtlasData {
    &ATLAS
}

/// Load a manifest PNG through the asset roots and resample it to one 16×16
/// tile. A vertical strip (h > w, h % w == 0) contributes its FIRST frame.
fn load_16(file: &str) -> Result<image::RgbaImage, String> {
    let rel = format!("textures/{file}");
    let (bytes, _) = crate::assets::read_bytes(&rel)
        .ok_or_else(|| format!("missing texture '{rel}' (searched the asset roots)"))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("failed to decode '{rel}': {e}"))?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let frame = if w > 0 && h > w && h % w == 0 {
        image::imageops::crop_imm(&img, 0, 0, w, w).to_image()
    } else {
        img
    };
    Ok(image::imageops::resize(
        &frame,
        TILE,
        TILE,
        image::imageops::FilterType::Nearest,
    ))
}

fn build(manifests: &[&str]) -> Result<AtlasData, String> {
    // Merge manifest layers by tile name: a later layer's row REPLACES the
    // earlier one (keeping its position, so replacement never renumbers ids
    // within a run); unknown names APPEND as new tiles. A pack can thus both
    // reskin a tile (with its own PNG resolved point-file-first anyway) and
    // add brand-new tiles for its item sprites.
    let mut rows: Vec<RawTile> = Vec::new();
    for (li, manifest) in manifests.iter().enumerate() {
        let raw: RawManifest = serde_json::from_str(manifest)
            .map_err(|e| format!("layer #{li}: invalid JSON: {e}"))?;
        for t in raw.tiles {
            match rows.iter_mut().find(|r| r.name == t.name) {
                Some(slot) => *slot = t,
                None => rows.push(t),
            }
        }
    }
    let raw = RawManifest { tiles: rows };

    // Expand the manifest into the flat ordered cell list (animated strips
    // contribute one cell per frame), carrying each cell's columns.
    struct Cell {
        name: String,
        pixels: image::RgbaImage,
        anim_frames: u32, // on the base frame of an animated tile; 0 otherwise
        tint: Option<TileTint>,
        icon_tint: Option<TileTint>,
        fill_cutout_mips: bool,
    }
    let mut cells: Vec<Cell> = Vec::new();
    for t in &raw.tiles {
        if !t.anim {
            cells.push(Cell {
                name: t.name.clone(),
                pixels: load_16(&t.file)?,
                anim_frames: 0,
                tint: t.tint,
                icon_tint: t.icon_tint,
                fill_cutout_mips: t.fill_cutout_mips,
            });
            continue;
        }
        let rel = format!("textures/{}", t.file);
        let (bytes, _) = crate::assets::read_bytes(&rel)
            .ok_or_else(|| format!("missing texture '{rel}' (searched the asset roots)"))?;
        let strip = image::load_from_memory(&bytes)
            .map_err(|e| format!("failed to decode '{rel}': {e}"))?
            .to_rgba8();
        let (sw, sh) = (strip.width(), strip.height());
        if sw == 0 || sh == 0 || sh % sw != 0 {
            return Err(format!(
                "animated texture '{rel}' must be a vertical strip of square frames, got {sw}x{sh}"
            ));
        }
        let frames = sh / sw;
        for i in 0..frames {
            let frame = image::imageops::crop_imm(&strip, 0, i * sw, sw, sw).to_image();
            let pixels =
                image::imageops::resize(&frame, TILE, TILE, image::imageops::FilterType::Nearest);
            cells.push(Cell {
                name: if i == 0 {
                    t.name.clone()
                } else {
                    format!("{}_{i}", t.name)
                },
                pixels,
                anim_frames: if i == 0 { frames } else { 0 },
                tint: t.tint,
                icon_tint: t.icon_tint,
                fill_cutout_mips: t.fill_cutout_mips,
            });
        }
    }

    let count = cells.len();
    // The packed chunk vertex carries the tile id in 8 bits and the shader's
    // uv-rect table is sized to match (`render::uniforms::UV_RECTS_LEN`), so
    // the atlas cannot exceed 256 tiles without a vertex-format change.
    if count > 256 {
        return Err(format!(
            "atlas has {count} tiles; the packed chunk vertex stores tile ids in 8 bits (max 256 — see render::uniforms::UV_RECTS_LEN)"
        ));
    }

    // Square-ish atlas grid, same shape rule the old build-time composer used.
    let cols = (count as f32).sqrt().ceil() as u32;
    let rows = (count as u32).div_ceil(cols);
    let atlas_w = cols * TILE;
    let atlas_h = rows * TILE;
    let mut rgba = vec![0u8; (atlas_w * atlas_h * 4) as usize];

    let mut names = Vec::with_capacity(count);
    let mut by_name: HashMap<&'static str, Tile> = HashMap::with_capacity(count);
    let mut anim_frames = Vec::with_capacity(count);
    let mut world_tint = Vec::with_capacity(count);
    let mut icon_tint = Vec::with_capacity(count);
    let mut fill_cutout_mips = Vec::with_capacity(count);
    let mut map_rgb = Vec::with_capacity(count);
    let mut min_alpha = Vec::with_capacity(count);
    for (i, cell) in cells.iter().enumerate() {
        let base_x = (i as u32 % cols) * TILE;
        let base_y = (i as u32 / cols) * TILE;
        let mut tile_min_alpha = u8::MAX;
        for y in 0..TILE {
            for x in 0..TILE {
                let px = cell.pixels.get_pixel(x, y);
                let dst = ((base_y + y) * atlas_w + base_x + x) as usize * 4;
                rgba[dst..dst + 4].copy_from_slice(&px.0);
                tile_min_alpha = tile_min_alpha.min(px.0[3]);
            }
        }
        min_alpha.push(tile_min_alpha);
        let name: &'static str = Box::leak(cell.name.clone().into_boxed_str());
        if by_name.insert(name, Tile(i as u16)).is_some() {
            return Err(format!("duplicate tile name '{name}'"));
        }
        names.push(name);
        anim_frames.push(cell.anim_frames);
        world_tint.push(cell.tint);
        icon_tint.push(cell.icon_tint);
        fill_cutout_mips.push(cell.fill_cutout_mips);
        map_rgb.push(tile_map_rgb(&cell.pixels));
    }

    let need = |name: &str| -> Result<Tile, String> {
        by_name
            .get(name)
            .copied()
            .ok_or_else(|| format!("engine tile '{name}' missing from the atlas manifest"))
    };
    let mut destroy_stages = [Tile(0); 10];
    for (i, slot) in destroy_stages.iter_mut().enumerate() {
        *slot = need(&format!("destroy_stage_{i}"))?;
    }
    let engine = EngineTiles {
        water_still: need("water_still")?,
        water_flow: need("water_flow")?,
        grass_side: need("grass_side")?,
        grass_side_overlay: need("grass_side_overlay")?,
        grass_snow: need("grass_snow")?,
        dirt: need("dirt")?,
        furnace_side: need("furnace_side")?,
        furnace_front: need("furnace_front")?,
        furnace_front_on: need("furnace_front_on")?,
        chest_top: need("chest_top")?,
        chest_front: need("chest_front")?,
        chest_side: need("chest_side")?,
        chest_lid_front: need("chest_lid_front")?,
        chest_lid_side: need("chest_lid_side")?,
        chest_inside: need("chest_inside")?,
        chest_latch: need("chest_latch")?,
        destroy_stages,
    };

    Ok(AtlasData {
        count,
        cols,
        rows,
        names,
        by_name,
        anim_frames,
        world_tint,
        icon_tint,
        fill_cutout_mips,
        map_rgb,
        min_alpha,
        rgba,
        engine,
    })
}

fn tile_map_rgb(pixels: &image::RgbaImage) -> [u8; 3] {
    let mut sum = [0u64; 3];
    let mut weight = 0u64;
    for pixel in pixels.pixels() {
        let a = pixel[3] as u64;
        if a < 16 {
            continue;
        }
        for channel in 0..3 {
            sum[channel] += pixel[channel] as u64 * a;
        }
        weight += a;
    }
    match sum.map(|channel| channel.checked_div(weight)) {
        [Some(r), Some(g), Some(b)] => [r as u8, g as u8, b as u8],
        _ => [32, 32, 32],
    }
}

// ---------------------------------------------------------------------------------
// Atlas pixel access + mips (unchanged consumers: render::resources, tile alpha)
// ---------------------------------------------------------------------------------

/// The composed atlas with a tile-isolated mip chain. The texture atlas uses
/// full-tile UVs, so generating mips over the whole atlas would bleed unrelated
/// tiles together. Tiles flagged `fill_cutout_mips` (leaves) get alpha expansion
/// while downsampling so distant cutout gaps fill with nearby colour instead of
/// disappearing under the shader's alpha test.
pub fn decode_atlas_mips() -> (Vec<Vec<u8>>, u32, u32) {
    let d = data();
    (build_atlas_mips(&d.rgba), d.cols * TILE, d.rows * TILE)
}

/// Per-tile texture-ARRAY data for the terrain pipeline: one `TILE×TILE` layer per tile id,
/// with a per-layer mip chain. Returned as `(levels, tile_size, layer_count)` where
/// `levels[mip]` is layer-major packed RGBA (`layer_count × (tile>>mip)² × 4` bytes) — one
/// `write_texture` per mip. Extracted from the same tile-isolated mips [`build_atlas_mips`]
/// builds (so leaf alpha-expansion etc. carry over), but repacked per layer so the array can
/// use real REPEAT wrapping + mips with NO cross-tile bleed — exactly what a greedy-meshed
/// quad's tiled UVs need. Layer index == tile id, matching the `uv_rects` / mesher numbering.
pub fn decode_atlas_array() -> (Vec<Vec<u8>>, u32, u32) {
    let d = data();
    let mips = build_atlas_mips(&d.rgba);
    let layers = d.count as u32;
    let mut levels = Vec::with_capacity(mips.len());
    for (level, mip) in mips.iter().enumerate() {
        let t = (TILE >> level).max(1) as usize;
        let mip_w = d.cols as usize * t;
        let row_bytes = t * 4;
        let mut buf = vec![0u8; layers as usize * t * t * 4];
        for tile in Tile::all() {
            let (col, row) = tile.grid();
            let (col, row) = (col as usize, row as usize);
            let layer = tile.index();
            for y in 0..t {
                let src = ((row * t + y) * mip_w + col * t) * 4;
                let dst = (layer * t * t + y * t) * 4;
                buf[dst..dst + row_bytes].copy_from_slice(&mip[src..src + row_bytes]);
            }
        }
        levels.push(buf);
    }
    (levels, TILE, layers)
}

fn build_atlas_mips(base: &[u8]) -> Vec<Vec<u8>> {
    let d = data();
    let levels = TILE.trailing_zeros() as usize + 1;
    let mut mips = Vec::with_capacity(levels);
    mips.push(base.to_vec());

    for level in 1..levels {
        let src_tile = (TILE >> (level - 1)) as usize;
        let dst_tile = (TILE >> level) as usize;
        let src_w = d.cols as usize * src_tile;
        let dst_w = d.cols as usize * dst_tile;
        let dst_h = d.rows as usize * dst_tile;
        let mut dst = vec![0u8; dst_w * dst_h * 4];

        for tile in Tile::all() {
            let (tile_col, tile_row) = tile.grid();
            let tile_col = tile_col as usize;
            let tile_row = tile_row as usize;
            for y in 0..dst_tile {
                for x in 0..dst_tile {
                    let px = downsample_mip_pixel(
                        &mips[level - 1],
                        src_w,
                        tile_col * src_tile + x * 2,
                        tile_row * src_tile + y * 2,
                        d.fill_cutout_mips[tile.index()],
                    );
                    let di = ((tile_row * dst_tile + y) * dst_w + tile_col * dst_tile + x) * 4;
                    dst[di..di + 4].copy_from_slice(&px);
                }
            }
        }

        debug_assert_eq!(dst.len(), dst_w * dst_h * 4);
        mips.push(dst);
    }

    mips
}

fn downsample_mip_pixel(
    src: &[u8],
    src_w: usize,
    x: usize,
    y: usize,
    fill_cutout: bool,
) -> [u8; 4] {
    let mut rgb = [0u32; 3];
    let mut alpha_sum = 0u32;
    let mut opaque_rgb = [0u32; 3];
    let mut opaque_count = 0u32;

    for sy in 0..2 {
        for sx in 0..2 {
            let si = ((y + sy) * src_w + x + sx) * 4;
            let r = src[si] as u32;
            let g = src[si + 1] as u32;
            let b = src[si + 2] as u32;
            let a = src[si + 3] as u32;

            alpha_sum += a;
            if a > 0 {
                rgb[0] += r * a;
                rgb[1] += g * a;
                rgb[2] += b * a;
            }
            if a >= 128 {
                opaque_rgb[0] += r;
                opaque_rgb[1] += g;
                opaque_rgb[2] += b;
                opaque_count += 1;
            }
        }
    }

    if fill_cutout && opaque_count > 0 {
        return [
            div_round(opaque_rgb[0], opaque_count),
            div_round(opaque_rgb[1], opaque_count),
            div_round(opaque_rgb[2], opaque_count),
            255,
        ];
    }

    if alpha_sum == 0 {
        return [0, 0, 0, 0];
    }

    [
        div_round(rgb[0], alpha_sum),
        div_round(rgb[1], alpha_sum),
        div_round(rgb[2], alpha_sum),
        div_round(alpha_sum, 4),
    ]
}

#[inline]
fn div_round(n: u32, d: u32) -> u8 {
    ((n + d / 2) / d).min(255) as u8
}

/// Packs the animated-water flipbook control for the block shader's `water_anim`
/// uniform: `(still_base_tile, flow_base_tile, frame_count, 0)`. The two bases
/// are the tile ids the mesher assigns to still/flow water tops & sides; the
/// shader cycles `base + frame` over `frame_count` consecutive atlas tiles.
pub fn water_anim_uniform() -> [u32; 4] {
    let e = engine();
    [
        e.water_still.index() as u32,
        e.water_flow.index() as u32,
        e.water_still.anim_frames(),
        0,
    ]
}

/// Tile grid -> normalized UV rect (u0,v0,u1,v1) for a tile.
pub fn tile_uv(tile: Tile) -> [f32; 4] {
    let d = data();
    let (col, row) = tile.grid();
    let u0 = col as f32 / d.cols as f32;
    let v0 = row as f32 / d.rows as f32;
    let u1 = (col + 1) as f32 / d.cols as f32;
    let v1 = (row + 1) as f32 / d.rows as f32;
    // No inset. Mips are generated per tile, and the atlas sampler still uses
    // nearest texel filtering, so there is no cross-tile bilinear bleed to guard
    // against; a half-texel inset shrank the edge texels to half-width, making
    // every block boundary look offset/overlapping. Full-tile UVs sample all 16
    // texels at full width and tile seamlessly across blocks.
    [u0, v0, u1, v1]
}

// ---------------------------------------------------------------------------------
// Tile alpha (cutout raycast targeting + sprite bounds)
// ---------------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TileAlphaBounds {
    pub u_min: f32,
    pub u_max: f32,
    /// Bottom-up texture-space v, matching plant model vertical coordinates.
    pub v_min: f32,
    pub v_max: f32,
}

struct TileAlphaData {
    rows: Vec<[u16; TILE_SIZE]>,
    bounds: Vec<Option<TileAlphaBounds>>,
}

static TILE_ALPHA: OnceLock<TileAlphaData> = OnceLock::new();

/// True when a bottom-up tile coordinate lands on a texel that survives the
/// cutout alpha test used by `fs_opaque`.
pub fn tile_alpha_opaque(tile: Tile, u: f32, v_bottom_up: f32) -> bool {
    let alpha = tile_alpha_data();
    let x = texel_coord(u);
    let y = texel_coord(1.0 - v_bottom_up);
    alpha.rows[tile.index()][y] & (1u16 << x) != 0
}

pub fn tile_alpha_bounds(tile: Tile) -> Option<TileAlphaBounds> {
    tile_alpha_data().bounds[tile.index()]
}

fn tile_alpha_data() -> &'static TileAlphaData {
    TILE_ALPHA.get_or_init(build_tile_alpha_data)
}

fn build_tile_alpha_data() -> TileAlphaData {
    let d = data();
    let w = (d.cols * TILE) as usize;
    let mut rows = vec![[0u16; TILE_SIZE]; d.count];
    let mut bounds = vec![None; d.count];

    for tile in Tile::all() {
        let (col, row) = tile.grid();
        let base_x = (col * TILE) as usize;
        let base_y = (row * TILE) as usize;
        let mut min_x = TILE_SIZE;
        let mut min_y = TILE_SIZE;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        let mut any = false;

        #[allow(clippy::needless_range_loop)]
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let i = ((base_y + y) * w + base_x + x) * 4;
                if d.rgba[i + 3] >= ALPHA_CUTOFF {
                    rows[tile.index()][y] |= 1u16 << x;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                    any = true;
                }
            }
        }

        if any {
            bounds[tile.index()] = Some(TileAlphaBounds {
                u_min: min_x as f32 / TILE_SIZE as f32,
                u_max: (max_x + 1) as f32 / TILE_SIZE as f32,
                v_min: (TILE_SIZE - max_y - 1) as f32 / TILE_SIZE as f32,
                v_max: (TILE_SIZE - min_y) as f32 / TILE_SIZE as f32,
            });
        }
    }

    TileAlphaData { rows, bounds }
}

fn texel_coord(v: f32) -> usize {
    (v.clamp(0.0, 1.0 - f32::EPSILON) * TILE_SIZE as f32).floor() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_loads_and_engine_tiles_resolve() {
        // Forces the LazyLock: a bad manifest/texture set panics right here.
        let d = data();
        assert!(d.count > 0 && d.count <= 256);
        assert_eq!(d.rgba.len(), (d.cols * TILE * d.rows * TILE * 4) as usize);
        // Names round-trip.
        for tile in Tile::all() {
            assert_eq!(Tile::from_name(tile.name()), Some(tile));
        }
        // Water animates; the two bases resolve.
        assert!(engine().water_still.anim_frames() > 0);
        assert!(engine().water_flow.anim_frames() > 0);
    }

    #[test]
    fn mips_are_tile_isolated_and_stop_at_one_texel_per_tile() {
        let d = data();
        let mips = build_atlas_mips(&d.rgba);

        assert_eq!(mips.len(), TILE.trailing_zeros() as usize + 1);

        for (level, mip) in mips.iter().enumerate() {
            let tile = TILE >> level;
            assert_eq!(mip.len(), (d.cols * tile * d.rows * tile * 4) as usize);
        }
        assert_eq!(TILE >> (mips.len() - 1), 1);
    }

    #[test]
    fn leaf_mips_expand_cutout_alpha() {
        let d = data();
        let leaves = Tile::from_name("oak_leaves").expect("oak_leaves tile");
        assert!(
            d.fill_cutout_mips[leaves.index()],
            "oak_leaves must carry fill_cutout_mips"
        );
        let mut base = vec![0u8; (d.cols * TILE * d.rows * TILE * 4) as usize];
        let (col, row) = leaves.grid();
        let leaf_x = col * TILE;
        let leaf_y = row * TILE;
        let i = ((leaf_y * d.cols * TILE + leaf_x) * 4) as usize;
        base[i..i + 4].copy_from_slice(&[30, 90, 20, 255]);

        let mips = build_atlas_mips(&base);
        let level1_w = (d.cols * (TILE / 2)) as usize;
        let level1_tile = (TILE / 2) as usize;
        let li = ((row as usize * level1_tile) * level1_w + col as usize * level1_tile) * 4;

        assert_eq!(&mips[1][li..li + 4], &[30, 90, 20, 255]);
    }

    #[test]
    fn manifest_layers_merge_by_tile_name() {
        let (base, _) = crate::assets::read_base_text("textures/atlas.json")
            .expect("assets/textures/atlas.json must ship");
        // A pack layer retints an existing tile (replacing its row in place)
        // and appends a brand-new tile reusing an existing PNG.
        let layer = r#"{"tiles": [{"name": "stone", "file": "stone.png", "tint": "grass"}, {"name": "test_extra_tile", "file": "stone.png"}]}"#;
        let d = build(&[&base, layer]).expect("layered manifest builds");
        let stone = d.by_name["stone"];
        assert_eq!(d.world_tint[stone.index()], Some(TileTint::Grass));
        let extra = d.by_name["test_extra_tile"];
        assert_eq!(extra.index(), d.count - 1, "new tiles append at the end");
    }

    #[test]
    fn tint_columns_mirror_the_engine_rules() {
        // In-world (mesher) tint classes.
        for name in ["grass_top", "short_grass", "fern"] {
            let t = Tile::from_name(name).unwrap();
            assert_eq!(t.world_tint(), Some(TileTint::Grass), "{name}");
        }
        for name in ["water", "water_still", "water_flow"] {
            let t = Tile::from_name(name).unwrap();
            assert_eq!(t.world_tint(), Some(TileTint::Water), "{name}");
        }
        for name in ["oak_leaves", "spruce_leaves", "redwood_leaves"] {
            let t = Tile::from_name(name).unwrap();
            assert_eq!(t.world_tint(), Some(TileTint::Foliage), "{name}");
        }
        // Azalea keeps its baked colour in the WORLD but green in icons.
        for name in ["azalea_leaves"] {
            let t = Tile::from_name(name).unwrap();
            assert_eq!(t.world_tint(), None, "{name}");
            assert_eq!(t.icon_tint(), Some(TileTint::Foliage), "{name}");
        }
        // icon_tint defaults to world_tint.
        let oak = Tile::from_name("oak_leaves").unwrap();
        assert_eq!(oak.icon_tint(), Some(TileTint::Foliage));
        let stone = Tile::from_name("stone").unwrap();
        assert_eq!(stone.world_tint(), None);
        assert_eq!(stone.icon_tint(), None);
    }
}
