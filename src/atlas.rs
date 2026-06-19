//! Atlas plumbing: load build-time PNG into RGBA bytes + tile lookup.

include!(concat!(env!("OUT_DIR"), "/atlas_data.rs"));

pub fn atlas_png_path() -> &'static str {
    env!("LLAMACRAFT_ATLAS_PNG")
}

/// Decode embedded atlas PNG into RGBA bytes (atlas_w*atlas_h*4).
pub fn decode_atlas() -> (Vec<u8>, u32, u32) {
    let bytes = std::include_bytes!(concat!(env!("OUT_DIR"), "/atlas.png"));
    let img = image::load_from_memory(bytes)
        .expect("decode atlas")
        .to_rgba8();
    let w = img.width();
    let h = img.height();
    (img.into_raw(), w, h)
}

/// Decode the atlas and build a tile-isolated mip chain. The texture atlas uses
/// full-tile UVs, so generating mips over the whole atlas would bleed unrelated
/// tiles together. Leaves get alpha expansion while downsampling so distant
/// cutout gaps fill with nearby leaf colour instead of disappearing under the
/// shader's alpha test.
pub fn decode_atlas_mips() -> (Vec<Vec<u8>>, u32, u32) {
    let (rgba, w, h) = decode_atlas();
    (build_atlas_mips(&rgba), w, h)
}

fn build_atlas_mips(base: &[u8]) -> Vec<Vec<u8>> {
    let levels = TILE.trailing_zeros() as usize + 1;
    let mut mips = Vec::with_capacity(levels);
    mips.push(base.to_vec());

    for level in 1..levels {
        let src_tile = (TILE >> (level - 1)) as usize;
        let dst_tile = (TILE >> level) as usize;
        let src_w = ATLAS_COLS as usize * src_tile;
        let dst_w = ATLAS_COLS as usize * dst_tile;
        let dst_h = ATLAS_ROWS as usize * dst_tile;
        let mut dst = vec![0u8; dst_w * dst_h * 4];

        for &tile in Tile::ALL {
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
                        tile == Tile::OakLeaves,
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

/// Tile grid -> normalized UV rect (u0,v0,u1,v1) for a tile.
pub fn tile_uv(tile: Tile) -> [f32; 4] {
    let (col, row) = tile.grid();
    let u0 = col as f32 / ATLAS_COLS as f32;
    let v0 = row as f32 / ATLAS_ROWS as f32;
    let u1 = (col + 1) as f32 / ATLAS_COLS as f32;
    let v1 = (row + 1) as f32 / ATLAS_ROWS as f32;
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

    #[test]
    fn mips_are_tile_isolated_and_stop_at_one_texel_per_tile() {
        let (base, w, h) = decode_atlas();
        let mips = build_atlas_mips(&base);

        assert_eq!(w, ATLAS_COLS * TILE);
        assert_eq!(h, ATLAS_ROWS * TILE);
        assert_eq!(mips.len(), TILE.trailing_zeros() as usize + 1);

        for (level, mip) in mips.iter().enumerate() {
            let tile = TILE >> level;
            assert_eq!(
                mip.len(),
                (ATLAS_COLS * tile * ATLAS_ROWS * tile * 4) as usize
            );
        }
        assert_eq!(TILE >> (mips.len() - 1), 1);
    }

    #[test]
    fn leaf_mips_expand_cutout_alpha() {
        let mut base = vec![0u8; (ATLAS_COLS * TILE * ATLAS_ROWS * TILE * 4) as usize];
        let (col, row) = Tile::OakLeaves.grid();
        let leaf_x = col * TILE;
        let leaf_y = row * TILE;
        let i = ((leaf_y * ATLAS_COLS * TILE + leaf_x) * 4) as usize;
        base[i..i + 4].copy_from_slice(&[30, 90, 20, 255]);

        let mips = build_atlas_mips(&base);
        let level1_w = (ATLAS_COLS * (TILE / 2)) as usize;
        let level1_tile = (TILE / 2) as usize;
        let li =
            (((row as usize * level1_tile) * level1_w + col as usize * level1_tile) * 4) as usize;

        assert_eq!(&mips[1][li..li + 4], &[30, 90, 20, 255]);
    }
}
