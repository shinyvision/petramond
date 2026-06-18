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

/// Tile grid -> normalized UV rect (u0,v0,u1,v1) for a tile.
pub fn tile_uv(tile: Tile) -> [f32; 4] {
    let (col, row) = tile.grid();
    let u0 = col as f32 / ATLAS_COLS as f32;
    let v0 = row as f32 / ATLAS_ROWS as f32;
    let u1 = (col + 1) as f32 / ATLAS_COLS as f32;
    let v1 = (row + 1) as f32 / ATLAS_ROWS as f32;
    // Inset slightly to avoid bilinear bleed at tile borders.
    let inset = 0.5 / ATLAS_W as f32;
    [u0 + inset, v0 + inset, u1 - inset, v1 - inset]
}