//! The HUD status-effect icon strip, CPU-composed once at renderer construction.
//!
//! Every registered effect (engine rows and pack rows alike — the registry is
//! session-stable) gets one [`CELL_PX`]² cell in a horizontal strip, id-ordered
//! so the UI can index cells by `Effect(id)`. Each cell is the shared rounded
//! frame (`textures/gui/effect_frame.png`, the #11191F-with-border chrome every
//! effect carries) with the row's own `icon` PNG composited inside. Icons are
//! authored [`CELL_PX`]² and fill the cell (transparent pixels show the frame
//! chrome); smaller icons composite centered un-resized, so legacy 12×12 art
//! keeps its inset-in-frame look. Oversize icons are nearest-resized so a
//! sloppy pack icon still lands in the frame instead of breaking the strip.
//!
//! This is presentation-only composition of freely-editable textures — no
//! tests pin its bytes (testing-and-verification.md).

use image::RgbaImage;

/// One atlas cell side in texture px; the HUD draws cells at this size in
/// logical px (so art is 1:1 at `gui_scale` 1).
pub const CELL_PX: u32 = 16;

/// Compose the id-ordered effect strip, or `None` when the frame art is
/// missing (the HUD then simply draws no effect row — never a panic).
pub fn compose_atlas() -> Option<RgbaImage> {
    let defs = crate::effect::defs();
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
            // A frame with no icon still marks the effect's presence.
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
    let (bytes, _path) = crate::assets::read_bytes(rel)?;
    Some(image::load_from_memory(&bytes).ok()?.to_rgba8())
}

fn fit(img: RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        img
    } else {
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Nearest)
    }
}
