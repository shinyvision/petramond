//! Runtime texture atlas PIXELS: composed at startup from `assets/textures/`
//! for the tile identities `tile` assigned, plus per-tile pixel-derived data
//! (map colours, alpha bounds, mips, UV rects).
//!
//! Identity (names, ids, tints, frame counts) lives in [`petramond_world::tile`] and
//! never touches a texel; this module is the client half that actually decodes
//! the PNGs. Like the block/item tables, the composed atlas is load-bearing
//! (meshing, render, and icon drawing all resolve through it), so composition
//! validates fully and panics with a precise message rather than misrendering.

use std::collections::HashMap;
use std::sync::LazyLock;

use petramond_world::tile::{engine, Tile};

/// Fixed tile edge length in texels. Every atlas cell is `TILE × TILE`.
pub const TILE: u32 = 16;

const ALPHA_CUTOFF: u8 = 128;

struct AtlasData {
    count: usize,
    cols: u32,
    rows: u32,
    map_rgb: Vec<[u8; 3]>,
    /// Lowest alpha across the tile's texels — the asset↔shader contract
    /// check: an OPAQUE block row's tiles must survive the block shader's
    /// cutout (`block.wgsl` discards `a < 0.5`), or the block renders as a
    /// hole (the invisible-ice bug, 2026-07-16).
    #[cfg_attr(not(test), allow(dead_code))]
    min_alpha: Vec<u8>,
    /// Composed atlas, `cols*TILE × 2*rows*TILE` RGBA: the declared tiles in
    /// the TOP half, and below them every tile's DYE-BASE twin (desaturated,
    /// brightness-normalized — see [`dye_base_pixels`]) at the same (col, row).
    /// Shaders sample the twin whenever a vertex carries the dyed flag; the
    /// twin set is universal on purpose — the engine stays agnostic to WHAT
    /// gets tinted, so every tile must be tintable.
    rgba: Vec<u8>,
}

static ATLAS: LazyLock<AtlasData> = LazyLock::new(|| {
    let data = compose().unwrap_or_else(|e| panic!("textures/atlas.json: {e}"));
    // Publish the pixel-derived cartography colours through the identity
    // registry, where headless-safe consumers (minimap surface tint) read them.
    petramond_world::tile::install_map_colors(data.map_rgb.clone());
    data
});

/// Force atlas composition now (and with it the tile map-colour install) —
/// for tools that read map colours without ever rendering.
pub fn ensure_composed() {
    LazyLock::force(&ATLAS);
}

#[inline]
fn data() -> &'static AtlasData {
    &ATLAS
}

/// (col, row) of a tile in the composed atlas grid.
#[inline]
pub fn tile_grid(tile: Tile) -> (u32, u32) {
    let d = data();
    (tile.id() as u32 % d.cols, tile.id() as u32 / d.cols)
}

/// Lowest alpha across this tile's texels — the asset↔render-pass
/// contract's input: an OPAQUE block row's tiles must be genuinely opaque
/// (≥ 128), a TRANSLUCENT row's tiles must author alpha in the 0.25..0.5
/// band (above the cutout passes' `a < 0.25` discard, below water's 0.5
/// split in `fs_transparent`). Pinned by
/// `block_tiles_match_their_render_pass_alpha_contract`.
#[cfg_attr(not(test), allow(dead_code))]
#[inline]
pub fn tile_min_alpha(tile: Tile) -> u8 {
    data().min_alpha[tile.index()]
}

/// Decode a source PNG once, full pixels. Animated strips stay whole here;
/// frames are cropped per cell.
fn load_source(file: &str) -> Result<image::RgbaImage, String> {
    let rel = format!("textures/{file}");
    let (bytes, _) = petramond_world::assets::read_bytes(&rel)
        .ok_or_else(|| format!("missing texture '{rel}' (searched the asset roots)"))?;
    Ok(image::load_from_memory(&bytes)
        .map_err(|e| format!("failed to decode '{rel}': {e}"))?
        .to_rgba8())
}

/// Compose the atlas pixels for the identity registry's cell list: each cell's
/// source frame resampled to `TILE × TILE`, placed at its id's grid slot, with
/// its dye-base twin one grid-half below.
fn compose() -> Result<AtlasData, String> {
    let cells = petramond_world::tile::cells();
    let count = cells.len();

    // Square-ish atlas grid, same shape rule the old build-time composer used.
    // The composed image is DOUBLE height: declared tiles on top, their
    // dye-base twins below (same col/row + `rows`).
    let cols = (count as f32).sqrt().ceil() as u32;
    let rows = (count as u32).div_ceil(cols);
    let atlas_w = cols * TILE;
    let atlas_h = 2 * rows * TILE;
    let mut rgba = vec![0u8; (atlas_w * atlas_h * 4) as usize];

    let mut sources: HashMap<&str, image::RgbaImage> = HashMap::new();
    let mut map_rgb = Vec::with_capacity(count);
    let mut min_alpha = Vec::with_capacity(count);
    for (i, cell) in cells.iter().enumerate() {
        let src = match sources.get(cell.file.as_str()) {
            Some(img) => img,
            None => {
                let img = load_source(&cell.file)?;
                sources.entry(cell.file.as_str()).or_insert(img)
            }
        };
        let (sw, sh) = (src.width(), src.height());
        // A vertical strip contributes the cell's frame; a static tile with
        // strip proportions contributes its first frame (legacy tolerance,
        // matching the identity registry's frame assignment).
        let frame = if sw > 0 && sh > sw && sh % sw == 0 {
            image::imageops::crop_imm(src, 0, cell.frame * sw, sw, sw).to_image()
        } else {
            src.clone()
        };
        let pixels =
            image::imageops::resize(&frame, TILE, TILE, image::imageops::FilterType::Nearest);

        let base_x = (i as u32 % cols) * TILE;
        let base_y = (i as u32 / cols) * TILE;
        let dye_y = base_y + rows * TILE;
        let dye = dye_base_pixels(&pixels);
        let mut tile_min_alpha = u8::MAX;
        for y in 0..TILE {
            for x in 0..TILE {
                let px = pixels.get_pixel(x, y);
                let dst = ((base_y + y) * atlas_w + base_x + x) as usize * 4;
                rgba[dst..dst + 4].copy_from_slice(&px.0);
                let dp = dye.get_pixel(x, y);
                let ddst = ((dye_y + y) * atlas_w + base_x + x) as usize * 4;
                rgba[ddst..ddst + 4].copy_from_slice(&dp.0);
                tile_min_alpha = tile_min_alpha.min(px.0[3]);
            }
        }
        min_alpha.push(tile_min_alpha);
        map_rgb.push(cell_map_rgb(&pixels));
    }

    Ok(AtlasData {
        count,
        cols,
        rows,
        map_rgb,
        min_alpha,
        rgba,
    })
}

/// The dye-base transform: desaturate to luminance, then scale so the
/// brightest visible texel hits 255. A tint multiply over the result can
/// reach the full dye color (white dye reads white); the base's own hue is
/// discarded but its texture detail survives in the luminance.
fn dye_base_pixels(src: &image::RgbaImage) -> image::RgbaImage {
    let luma = |p: &image::Rgba<u8>| {
        0.2126 * p.0[0] as f32 + 0.7152 * p.0[1] as f32 + 0.0722 * p.0[2] as f32
    };
    let max = src
        .pixels()
        .filter(|p| p.0[3] >= ALPHA_CUTOFF)
        .map(&luma)
        .fold(0.0f32, f32::max);
    let scale = if max > 0.0 { 255.0 / max } else { 1.0 };
    let mut out = src.clone();
    for p in out.pixels_mut() {
        let v = (luma(p) * scale).round().min(255.0) as u8;
        p.0 = [v, v, v, p.0[3]];
    }
    out
}

fn cell_map_rgb(pixels: &image::RgbaImage) -> [u8; 3] {
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
    (build_atlas_mips(&d.rgba), d.cols * TILE, 2 * d.rows * TILE)
}

/// The normalized V offset from a tile's base rect ([`tile_uv`]) to its
/// dye-base twin in the composed 2D atlas — exactly half, because the twin
/// half doubles the height. Shaders/CPU paths add this when a
/// `petramond:tint` multiply applies (`block.wgsl` mirrors the same rule as a
/// layer offset on the texture array).
pub const DYE_V_OFFSET: f32 = 0.5;

/// Per-tile texture-ARRAY data for the terrain pipeline: one `TILE×TILE` layer per tile id,
/// with a per-layer mip chain. Returned as `(levels, tile_size, layer_count)` where
/// `levels[mip]` is layer-major packed RGBA (`layer_count × (tile>>mip)² × 4` bytes) — one
/// `write_texture` per mip. Extracted from the same tile-isolated mips `build_atlas_mips`
/// builds (so leaf alpha-expansion etc. carry over), but repacked per layer so the array can
/// use real REPEAT wrapping + mips with NO cross-tile bleed — exactly what a greedy-meshed
/// quad's tiled UVs need. Layer index == tile id, matching the `uv_rects` / mesher numbering.
pub fn decode_atlas_array() -> (Vec<Vec<u8>>, u32, u32) {
    let d = data();
    let mips = build_atlas_mips(&d.rgba);
    // Base layers `0..count`, then every tile's dye-base twin at
    // `count + tile` — the layer offset `block.wgsl` adds for dyed vertices.
    let layers = 2 * d.count as u32;
    let mut levels = Vec::with_capacity(mips.len());
    for (level, mip) in mips.iter().enumerate() {
        let t = (TILE >> level).max(1) as usize;
        let mip_w = d.cols as usize * t;
        let row_bytes = t * 4;
        let mut buf = vec![0u8; layers as usize * t * t * 4];
        for tile in Tile::all() {
            let (col, row) = tile_grid(tile);
            let (col, row) = (col as usize, row as usize);
            for (layer, row) in [
                (tile.index(), row),
                (d.count + tile.index(), row + d.rows as usize),
            ] {
                for y in 0..t {
                    let src = ((row * t + y) * mip_w + col * t) * 4;
                    let dst = (layer * t * t + y * t) * 4;
                    buf[dst..dst + row_bytes].copy_from_slice(&mip[src..src + row_bytes]);
                }
            }
        }
        levels.push(buf);
    }
    (levels, TILE, layers)
}

fn build_atlas_mips(base: &[u8]) -> Vec<Vec<u8>> {
    let d = data();
    let cells = petramond_world::tile::cells();
    let levels = TILE.trailing_zeros() as usize + 1;
    let mut mips = Vec::with_capacity(levels);
    mips.push(base.to_vec());

    for level in 1..levels {
        let src_tile = (TILE >> (level - 1)) as usize;
        let dst_tile = (TILE >> level) as usize;
        let src_w = d.cols as usize * src_tile;
        let dst_w = d.cols as usize * dst_tile;
        let dst_h = 2 * d.rows as usize * dst_tile;
        let mut dst = vec![0u8; dst_w * dst_h * 4];

        for tile in Tile::all() {
            let (tile_col, tile_row) = tile_grid(tile);
            let tile_col = tile_col as usize;
            // The tile's base cell, then its dye-base twin one grid-half down.
            for tile_row in [tile_row as usize, tile_row as usize + d.rows as usize] {
                for y in 0..dst_tile {
                    for x in 0..dst_tile {
                        let px = downsample_mip_pixel(
                            &mips[level - 1],
                            src_w,
                            tile_col * src_tile + x * 2,
                            tile_row * src_tile + y * 2,
                            cells[tile.index()].fill_cutout_mips,
                        );
                        let di = ((tile_row * dst_tile + y) * dst_w + tile_col * dst_tile + x) * 4;
                        dst[di..di + 4].copy_from_slice(&px);
                    }
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

/// Packs the animated-water flipbook control for the block shader's `atlas_anim`
/// uniform: `(still_base_tile, flow_base_tile, frame_count, 0)`. The two bases
/// are the tile ids the mesher assigns to still/flow water tops & sides; the
/// shader cycles `base + frame` over `frame_count` consecutive atlas tiles.
pub fn atlas_anim_uniform() -> [u32; 4] {
    let e = engine();
    [
        e.water_still.index() as u32,
        e.water_flow.index() as u32,
        e.water_still.anim_frames(),
        // `w`: the tile count — the texture-array layer offset from a tile to
        // its dye-base twin (`block.wgsl` adds it for dyed vertices).
        Tile::count() as u32,
    ]
}

/// Tile grid -> normalized UV rect (u0,v0,u1,v1) for a tile.
pub fn tile_uv(tile: Tile) -> [f32; 4] {
    let d = data();
    let (col, row) = tile_grid(tile);
    // V is normalized against the DOUBLE-height composed atlas (declared
    // tiles on top, dye-base twins below), so every base rect lands in the
    // top half and the dyed sample is exactly `v + 0.5` (see `DYE_V_OFFSET`).
    let u0 = col as f32 / d.cols as f32;
    let v0 = row as f32 / (2 * d.rows) as f32;
    let u1 = (col + 1) as f32 / d.cols as f32;
    let v1 = (row + 1) as f32 / (2 * d.rows) as f32;
    // No inset. Mips are generated per tile, and the atlas sampler still uses
    // nearest texel filtering, so there is no cross-tile bilinear bleed to guard
    // against; a half-texel inset shrank the edge texels to half-width, making
    // every block boundary look offset/overlapping. Full-tile UVs sample all 16
    // texels at full width and tile seamlessly across blocks.
    [u0, v0, u1, v1]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asset↔shader contract, both directions. OPAQUE rows: every referenced
    /// tile must be genuinely opaque (min alpha ≥ 128, comfortably above the
    /// cutout passes' 0.25 discard) or the block renders as an invisible
    /// x-ray hole — the mesher culled the faces behind it, then the shader
    /// discarded every texel of its own (the 2026-07-16 ice bug: `ice.png` at
    /// uniform alpha 126). TRANSLUCENT rows: tiles must sit in the 0.25..0.5
    /// band — at or above the cutout discard so item cubes/icons/particles
    /// still draw them solid, and below 0.5 so `fs_transparent`'s water/ice
    /// split hands them their own authored alpha instead of water's constant.
    #[test]
    fn block_tiles_match_their_render_pass_alpha_contract() {
        for &b in petramond_world::block::Block::all() {
            for tile in b.tiles() {
                if b.is_opaque() {
                    assert!(
                        tile_min_alpha(tile) >= 128,
                        "opaque {b:?} tile '{}' has sub-opaque texels (min alpha {})",
                        tile.name(),
                        tile_min_alpha(tile),
                    );
                } else if b.is_translucent() {
                    assert!(
                        (64..128).contains(&tile_min_alpha(tile)),
                        "translucent {b:?} tile '{}' must author alpha in 0.25..0.5 \
                         (min alpha {})",
                        tile.name(),
                        tile_min_alpha(tile),
                    );
                }
            }
        }
    }

    #[test]
    fn composed_atlas_matches_the_identity_registry() {
        // Forces the LazyLock: a bad texture set panics right here.
        let d = data();
        assert_eq!(d.count, Tile::count());
        assert_eq!(
            d.rgba.len(),
            (d.cols * TILE * 2 * d.rows * TILE * 4) as usize
        );
    }

    #[test]
    fn mips_are_tile_isolated_and_stop_at_one_texel_per_tile() {
        let d = data();
        let mips = build_atlas_mips(&d.rgba);

        assert_eq!(mips.len(), TILE.trailing_zeros() as usize + 1);

        for (level, mip) in mips.iter().enumerate() {
            let tile = TILE >> level;
            assert_eq!(mip.len(), (d.cols * tile * 2 * d.rows * tile * 4) as usize);
        }
        assert_eq!(TILE >> (mips.len() - 1), 1);
    }

    #[test]
    fn leaf_mips_expand_cutout_alpha() {
        let d = data();
        let leaves = Tile::from_name("oak_leaves").expect("oak_leaves tile");
        assert!(
            petramond_world::tile::cells()[leaves.index()].fill_cutout_mips,
            "oak_leaves must carry fill_cutout_mips"
        );
        let mut base = vec![0u8; (d.cols * TILE * 2 * d.rows * TILE * 4) as usize];
        let (col, row) = tile_grid(leaves);
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
}
