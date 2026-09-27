use image::RgbaImage;

pub const CELL_PX: u32 = 16;

pub fn compose_atlas() -> Option<RgbaImage> {
    let defs = petramond_world::effect::defs();
    let frame = load_rgba("textures/gui/effect_frame.png")?;
    let frame = fit(frame, CELL_PX, CELL_PX);
    let mut atlas = RgbaImage::new(CELL_PX * defs.len() as u32, CELL_PX);
    for def in defs {
        let x0 = def.effect.0 as i64 * CELL_PX as i64;
        image::imageops::overlay(&mut atlas, &frame, x0, 0);
        match load_rgba(def.icon) {
            Some(icon) => {
                let (w, h) = icon.dimensions();
                let icon = if w <= CELL_PX && h <= CELL_PX {
                    icon
                } else {
                    fit(icon, CELL_PX, CELL_PX)
                };
                let (w, h) = icon.dimensions();
                let (ix, iy) = ((CELL_PX - w) as i64 / 2, (CELL_PX - h) as i64 / 2);
                image::imageops::overlay(&mut atlas, &icon, x0 + ix, iy);
            }
            None => log::warn!(
                "effect '{}': icon '{}' missing or unreadable",
                def.name,
                def.icon
            ),
        }
    }
    Some(atlas)
}

fn load_rgba(rel: &str) -> Option<RgbaImage> {
    let (bytes, _path) = petramond_world::assets::read_bytes(rel)?;
    Some(image::load_from_memory(&bytes).ok()?.to_rgba8())
}

fn fit(img: RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        img
    } else {
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Nearest)
    }
}
