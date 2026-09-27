use std::path::{Path, PathBuf};

use image::{imageops, RgbaImage};

pub const MAX_INPUT: usize = 512 * 1024;
const MAX_SIDE: u32 = 2048;
pub const SIDE: u32 = 64;

pub fn normalize(bytes: &[u8]) -> Result<RgbaImage, String> {
    if bytes.len() > MAX_INPUT {
        return Err("the icon is larger than 512 KB".into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("the icon cannot be read: {e}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(64 << 20);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("the icon cannot be read: {e}"))?
        .to_rgba8();
    let (w, h) = image.dimensions();
    let long = w.max(h);
    let image = if long <= SIDE {
        image
    } else if long % SIDE == 0 && w % (long / SIDE) == 0 && h % (long / SIDE) == 0 {
        let factor = long / SIDE;
        imageops::resize(
            &image,
            w / factor,
            h / factor,
            imageops::FilterType::Nearest,
        )
    } else {
        let (nw, nh) = (
            ((w as u64 * SIDE as u64) / long as u64).max(1) as u32,
            ((h as u64 * SIDE as u64) / long as u64).max(1) as u32,
        );
        imageops::resize(&image, nw, nh, imageops::FilterType::Triangle)
    };
    let (w, h) = image.dimensions();
    let side = w.max(h);
    let mut square = RgbaImage::new(side, side);
    imageops::overlay(
        &mut square,
        &image,
        i64::from((side - w) / 2),
        i64::from((side - h) / 2),
    );
    Ok(square)
}

pub fn cache_path(dir: &Path, mod_id: &str, sha256: &str) -> PathBuf {
    dir.join(format!("{mod_id}-{sha256}.png"))
}

pub fn cache(dir: &Path, mod_id: &str, sha256: &str, bytes: &[u8]) -> Result<RgbaImage, String> {
    let image = normalize(bytes)?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = cache_path(dir, mod_id, sha256);
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    petramond_persist::atomic_file::replace(&path, &png).map_err(|e| e.to_string())?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        let prefix = format!("{mod_id}-");
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let stale = name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".png"))
                .is_some_and(|sha| sha.len() == 64 && sha != sha256);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(image)
}
