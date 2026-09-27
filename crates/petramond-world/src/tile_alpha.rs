//! Per-tile cutout ALPHA masks: which texels of a tile survive the render
//! passes' alpha test. SIM data, not presentation — the cutout raycast
//! (targeting through a plant sprite's transparent texels) must answer
//! identically on the client and the authoritative server, so this decodes
//! the alpha channel itself (headless included) instead of riding the
//! client's composed atlas.

use crate::assets::PackSet;
use crate::tile::Tile;

const TILE_SIZE: usize = 16;
const ALPHA_CUTOFF: u8 = 128;

fn cell_alpha(packs: &PackSet, file: &str, frame: u32) -> Result<image::RgbaImage, String> {
    let rel = format!("textures/{file}");
    let (bytes, _) = packs
        .read_bytes(&rel)
        .ok_or_else(|| format!("missing texture '{rel}' (searched the asset roots)"))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("failed to decode '{rel}': {e}"))?
        .to_rgba8();
    let (sw, sh) = (img.width(), img.height());
    let f = if sw > 0 && sh > sw && sh % sw == 0 {
        image::imageops::crop_imm(&img, 0, frame * sw, sw, sw).to_image()
    } else {
        img
    };
    Ok(image::imageops::resize(
        &f,
        TILE_SIZE as u32,
        TILE_SIZE as u32,
        image::imageops::FilterType::Nearest,
    ))
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TileAlphaBounds {
    pub u_min: f32,
    pub u_max: f32,
    pub v_min: f32,
    pub v_max: f32,
}

pub(crate) struct TileAlphaData {
    rows: Vec<[u16; TILE_SIZE]>,
    bounds: Vec<Option<TileAlphaBounds>>,
}

pub(crate) static TABLE: crate::content::Slot<TileAlphaData> = crate::content::Slot::new(
    crate::content::stage::TILE_ALPHA,
    &[crate::content::stage::TILES],
    build_tile_alpha_data,
);

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
    TABLE.current()
}

fn build_tile_alpha_data(reg: &crate::content::ContentRegistry) -> Result<TileAlphaData, String> {
    let cells = crate::tile::cells();
    let mut rows = vec![[0u16; TILE_SIZE]; cells.len()];
    let mut bounds = vec![None; cells.len()];
    let mut errors = Vec::new();

    for tile in Tile::all() {
        let cell = &cells[tile.index()];
        let pixels = match cell_alpha(reg.packs(), &cell.file, cell.frame) {
            Ok(pixels) => pixels,
            Err(e) => {
                errors.push(format!("tile '{}': {e}", cell.name));
                continue;
            }
        };
        let mut min_x = TILE_SIZE;
        let mut min_y = TILE_SIZE;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        let mut any = false;

        #[allow(clippy::needless_range_loop)]
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                if pixels.get_pixel(x as u32, y as u32).0[3] >= ALPHA_CUTOFF {
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

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(TileAlphaData { rows, bounds })
}

fn texel_coord(v: f32) -> usize {
    (v.clamp(0.0, 1.0 - f32::EPSILON) * TILE_SIZE as f32).floor() as usize
}
