//! Two jobs:
//!   1. `layer_regions` — turn a (fit-mode, source size, dest rect) into a list
//!      of (dest-rect, uv-rect) pieces. Used by BOTH the live canvas preview and
//!      the CPU bake, so what you see is what gets baked.
//!   2. `bake_to_files` — composite visible layers (+ optional slot frames) into
//!      a PNG and write the JSON manifest the game consumes.
//!
//! Model rects are whole pixels (`i32`); region geometry is computed in floats
//! (nine-slice insets can land mid-pixel after clamping) and rounded at blit.

use crate::assets::AssetLibrary;
use crate::model::{AssetSpec, Canvas, GuiType, Layer, LayerFit, LayerTag, Project, SlotRole};
use image::{imageops, imageops::FilterType, Rgba, RgbaImage};
use serde::Serialize;
use std::path::Path;

/// One piece to draw: a destination rect `[x, y, w, h]` (canvas units) sampling
/// a uv sub-rect `[u0, v0, u1, v1]` (0..1) of the source texture.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub dst: [f32; 4],
    pub uv: [f32; 4],
}

/// Decompose a layer placement into drawable regions, optionally flipped.
///
/// Flip mirrors both the piece *arrangement* (about the layer's centre) and the
/// sampled uv (by reversing it: u0>u1 / v0>v1), so nine-slice and tile flip
/// correctly rather than just a single sprite.
#[allow(clippy::too_many_arguments)]
pub fn layer_regions(
    fit: LayerFit,
    src_w: usize,
    src_h: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    flip_h: bool,
    flip_v: bool,
) -> Vec<Region> {
    let (xf, yf, wf, hf) = (x as f32, y as f32, w as f32, h as f32);
    let mut regs = match fit {
        LayerFit::Stretch => nine_slice_regions(src_w, src_h, 0, 0, 0, 0, xf, yf, wf, hf),
        LayerFit::NineSlice { l, r, t, b } => {
            nine_slice_regions(src_w, src_h, l, r, t, b, xf, yf, wf, hf)
        }
        LayerFit::Tile => tile_regions(src_w, src_h, xf, yf, wf, hf),
    };
    for reg in &mut regs {
        if flip_h {
            reg.dst[0] = 2.0 * xf + wf - reg.dst[0] - reg.dst[2];
            reg.uv.swap(0, 2);
        }
        if flip_v {
            reg.dst[1] = 2.0 * yf + hf - reg.dst[1] - reg.dst[3];
            reg.uv.swap(1, 3);
        }
    }
    regs
}

#[allow(clippy::too_many_arguments)]
fn nine_slice_regions(
    src_w: usize,
    src_h: usize,
    l: u32,
    r: u32,
    t: u32,
    b: u32,
    rx: f32,
    ry: f32,
    rw: f32,
    rh: f32,
) -> Vec<Region> {
    let sw = src_w.max(1) as f32;
    let sh = src_h.max(1) as f32;
    // Clamp insets so opposite pairs never overlap in source OR dest.
    let l = (l as f32).min(sw * 0.5).min(rw * 0.5).max(0.0);
    let r = (r as f32).min(sw * 0.5).min(rw * 0.5).max(0.0);
    let t = (t as f32).min(sh * 0.5).min(rh * 0.5).max(0.0);
    let b = (b as f32).min(sh * 0.5).min(rh * 0.5).max(0.0);

    let xs = [0.0, l, sw - r, sw];
    let ys = [0.0, t, sh - b, sh];
    let dx = [rx, rx + l, rx + rw - r, rx + rw];
    let dy = [ry, ry + t, ry + rh - b, ry + rh];

    let mut out = Vec::with_capacity(9);
    for j in 0..3 {
        for i in 0..3 {
            let (dw, dh) = (dx[i + 1] - dx[i], dy[j + 1] - dy[j]);
            if dw <= 0.01 || dh <= 0.01 {
                continue;
            }
            out.push(Region {
                dst: [dx[i], dy[j], dw, dh],
                uv: [xs[i] / sw, ys[j] / sh, xs[i + 1] / sw, ys[j + 1] / sh],
            });
        }
    }
    out
}

fn tile_regions(src_w: usize, src_h: usize, rx: f32, ry: f32, rw: f32, rh: f32) -> Vec<Region> {
    let sw = src_w.max(1) as f32;
    let sh = src_h.max(1) as f32;
    let mut out = Vec::new();
    let mut y = ry;
    let mut rows = 0;
    while y < ry + rh - 0.01 && rows < 4096 {
        let th = sh.min(ry + rh - y);
        let mut x = rx;
        let mut cols = 0;
        while x < rx + rw - 0.01 && cols < 4096 {
            let tw = sw.min(rx + rw - x);
            out.push(Region {
                dst: [x, y, tw, th],
                uv: [0.0, 0.0, tw / sw, th / sh],
            });
            x += sw;
            cols += 1;
        }
        y += sh;
        rows += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Baking
// ---------------------------------------------------------------------------

fn asset_image(lib: &AssetLibrary, spec: &AssetSpec) -> Result<RgbaImage, String> {
    let data = lib
        .get(spec)
        .ok_or_else(|| format!("asset not loaded: {spec:?}"))?;
    RgbaImage::from_raw(data.size[0] as u32, data.size[1] as u32, data.rgba.clone())
        .ok_or_else(|| "asset buffer size mismatch".to_string())
}

/// Blit one region of `src` into `canvas`, scaling to the dest rect (nearest,
/// for crisp pixels), honoring flipped uv (u0>u1 / v0>v1) and applying opacity.
fn blit_region(canvas: &mut RgbaImage, src: &RgbaImage, reg: &Region, opacity: f32) {
    let (sw, sh) = (src.width(), src.height());
    let flip_h = reg.uv[0] > reg.uv[2];
    let flip_v = reg.uv[1] > reg.uv[3];
    let (ua, ub) = (reg.uv[0].min(reg.uv[2]), reg.uv[0].max(reg.uv[2]));
    let (va, vb) = (reg.uv[1].min(reg.uv[3]), reg.uv[1].max(reg.uv[3]));
    let sx0 = (ua * sw as f32).round().clamp(0.0, sw as f32) as u32;
    let sx1 = (ub * sw as f32).round().clamp(0.0, sw as f32) as u32;
    let sy0 = (va * sh as f32).round().clamp(0.0, sh as f32) as u32;
    let sy1 = (vb * sh as f32).round().clamp(0.0, sh as f32) as u32;
    let (cw, ch) = (sx1.saturating_sub(sx0), sy1.saturating_sub(sy0));
    if cw == 0 || ch == 0 {
        return;
    }
    let dw = reg.dst[2].round().max(1.0) as u32;
    let dh = reg.dst[3].round().max(1.0) as u32;

    let cropped = imageops::crop_imm(src, sx0, sy0, cw, ch).to_image();
    let mut resized = imageops::resize(&cropped, dw, dh, FilterType::Nearest);
    if flip_h {
        resized = imageops::flip_horizontal(&resized);
    }
    if flip_v {
        resized = imageops::flip_vertical(&resized);
    }
    if opacity < 0.999 {
        for p in resized.pixels_mut() {
            p.0[3] = (p.0[3] as f32 * opacity).round().clamp(0.0, 255.0) as u8;
        }
    }
    imageops::overlay(
        canvas,
        &resized,
        reg.dst[0].round() as i64,
        reg.dst[1].round() as i64,
    );
}

/// Nearest-neighbor rotation of `src` clockwise by `angle` radians about its
/// centre. Returns a buffer sized to the rotated bounding box (transparent fill).
fn rotate_rgba(src: &RgbaImage, angle: f32) -> RgbaImage {
    let (sw, sh) = (src.width() as f32, src.height() as f32);
    let (sin, cos) = (angle.sin(), angle.cos());
    // Subtract a small epsilon before ceil so exact right angles (where cos/sin
    // carry fp noise, e.g. cos(90°) ≈ 4e-8) don't inflate the bounding box.
    let nw = (sw * cos.abs() + sh * sin.abs() - 1e-3).ceil().max(1.0);
    let nh = (sw * sin.abs() + sh * cos.abs() - 1e-3).ceil().max(1.0);
    let (nwu, nhu) = (nw as u32, nh as u32);
    let mut out = RgbaImage::from_pixel(nwu, nhu, Rgba([0, 0, 0, 0]));
    let (cx, cy) = (sw * 0.5, sh * 0.5);
    let (ocx, ocy) = (nw * 0.5, nh * 0.5);
    for oy in 0..nhu {
        for ox in 0..nwu {
            // Inverse-map the output pixel back into the source (rotate by -angle).
            let dx = ox as f32 + 0.5 - ocx;
            let dy = oy as f32 + 0.5 - ocy;
            let sx = cos * dx + sin * dy + cx;
            let sy = -sin * dx + cos * dy + cy;
            if sx >= 0.0 && sy >= 0.0 && sx < sw && sy < sh {
                let p = *src.get_pixel(sx as u32, sy as u32);
                if p.0[3] != 0 {
                    out.put_pixel(ox, oy, p);
                }
            }
        }
    }
    out
}

/// Render one layer to its own image plus the canvas-space top-left where it
/// should sit. Honors fit/flip/rotation/opacity, so a separately-baked overlay
/// lines up exactly with where it was authored on the panel.
fn render_layer(layer: &Layer, lib: &AssetLibrary) -> Result<(RgbaImage, i32, i32), String> {
    let src = asset_image(lib, &layer.asset)?;
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (lw, lh) = (layer.rect.w.max(1), layer.rect.h.max(1));
    let mut local = RgbaImage::from_pixel(lw as u32, lh as u32, Rgba([0, 0, 0, 0]));
    for reg in layer_regions(layer.fit, sw, sh, 0, 0, lw, lh, layer.flip_h, layer.flip_v) {
        blit_region(&mut local, &src, &reg, layer.opacity);
    }
    if layer.rotation.rem_euclid(360) == 0 {
        Ok((local, layer.rect.x, layer.rect.y))
    } else {
        // Rotate about the centre; offset so the centre stays put.
        let rotated = rotate_rgba(&local, (layer.rotation as f32).to_radians());
        let cx = layer.rect.x as f32 + lw as f32 * 0.5;
        let cy = layer.rect.y as f32 + lh as f32 * 0.5;
        let ox = (cx - rotated.width() as f32 * 0.5).round() as i32;
        let oy = (cy - rotated.height() as f32 * 0.5).round() as i32;
        Ok((rotated, ox, oy))
    }
}

/// Composite the project into an RGBA image at canvas resolution.
pub fn bake_png(project: &Project, lib: &AssetLibrary) -> Result<RgbaImage, String> {
    let w = project.canvas.w.max(1);
    let h = project.canvas.h.max(1);
    let mut canvas = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));

    for fl in project.flat_layers() {
        if !fl.effective_visible {
            continue;
        }
        // Tagged layers are dynamic overlays baked to their own PNGs, so they're
        // kept out of the static panel composite.
        if fl.layer.tag.is_some() {
            continue;
        }
        let (img, ox, oy) = render_layer(fl.layer, lib)?;
        imageops::overlay(&mut canvas, &img, ox as i64, oy as i64);
    }

    // Slot frames are baked separately so a slot's logical position and its
    // painted frame can never drift apart.
    let frame_spec = AssetSpec::Builtin {
        key: "slot".to_string(),
    };
    if project.slots.iter().any(|s| s.paint_frame) {
        let frame = asset_image(lib, &frame_spec)?;
        for slot in &project.slots {
            if !slot.paint_frame {
                continue;
            }
            for cell in slot.cells() {
                let reg = Region {
                    dst: [cell.x as f32, cell.y as f32, cell.w as f32, cell.h as f32],
                    uv: [0.0, 0.0, 1.0, 1.0],
                };
                blit_region(&mut canvas, &frame, &reg, 1.0);
            }
        }
    }

    Ok(canvas)
}

// ---- JSON manifest the game reads ----------------------------------------

#[derive(Serialize)]
struct ManifestSlot {
    role: SlotRole,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// The hover highlight in the manifest: the graphic (a sibling PNG) to draw over
/// a hovered slot, inflated by `margin` (canvas px) on every side.
#[derive(Serialize)]
struct HoverManifest {
    image: String,
    margin: i32,
    fit: LayerFit,
    opacity: f32,
}

/// A tagged overlay in the manifest: a sibling PNG the game draws at (x, y) and
/// drives at runtime (e.g. clipping by smelt progress / remaining fuel). `w`/`h`
/// are the PNG's pixel size; (x, y) is its top-left in canvas units.
#[derive(Serialize)]
struct TaggedManifest {
    tag: LayerTag,
    image: String,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    fit: LayerFit,
}

#[derive(Serialize)]
struct Manifest {
    #[serde(rename = "type")]
    gui_type: GuiType,
    canvas: Canvas,
    scale: u32,
    image: String,
    slots: Vec<ManifestSlot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hover: Option<HoverManifest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tagged: Vec<TaggedManifest>,
}

fn build_manifest(
    project: &Project,
    image_name: String,
    hover: Option<HoverManifest>,
    tagged: Vec<TaggedManifest>,
) -> Manifest {
    let mut slots = Vec::new();
    for slot in &project.slots {
        for cell in slot.cells() {
            slots.push(ManifestSlot {
                role: slot.role,
                x: cell.x,
                y: cell.y,
                w: cell.w,
                h: cell.h,
            });
        }
    }
    Manifest {
        gui_type: project.gui_type,
        canvas: project.canvas,
        scale: project.scale,
        image: image_name,
        slots,
        hover,
        tagged,
    }
}

/// Bake the project to `<name>.png` + `<name>.json` at the chosen png path.
pub fn bake_to_files(project: &Project, lib: &AssetLibrary, png_path: &Path) -> Result<(), String> {
    let img = bake_png(project, lib)?;
    let png_path = if png_path.extension().is_some() {
        png_path.to_path_buf()
    } else {
        png_path.with_extension("png")
    };
    img.save(&png_path).map_err(|e| format!("write png: {e}"))?;

    let stem = png_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "gui".to_string());

    // The hover highlight (if any) is a SEPARATE PNG next to the panel — it's
    // drawn dynamically on hover, so it can't be composited into the static panel.
    let hover = if let Some(h) = &project.hover {
        let src = asset_image(lib, &h.asset)?;
        let hover_name = format!("{stem}_hover.png");
        let hover_path = png_path.with_file_name(&hover_name);
        src.save(&hover_path)
            .map_err(|e| format!("write hover png: {e}"))?;
        Some(HoverManifest {
            image: hover_name,
            margin: h.margin,
            fit: h.fit,
            opacity: h.opacity,
        })
    } else {
        None
    };

    // Tagged layers (furnace arrow/flame, …) each bake to their own sibling PNG
    // and are recorded in the manifest, so the game can draw them dynamically.
    // Baked regardless of the builder visibility toggle — hiding one just
    // declutters the canvas preview; it still has a position to record.
    let mut tagged = Vec::new();
    for fl in project.flat_layers() {
        let Some(tag) = fl.layer.tag else {
            continue;
        };
        let (overlay, x, y) = render_layer(fl.layer, lib)?;
        let (w, h) = (overlay.width(), overlay.height());
        let name = format!("{stem}_{}.png", tag.key());
        overlay
            .save(png_path.with_file_name(&name))
            .map_err(|e| format!("write tagged png: {e}"))?;
        tagged.push(TaggedManifest {
            tag,
            image: name,
            x,
            y,
            w,
            h,
            fit: fl.layer.fit,
        });
    }

    let json_path = png_path.with_extension("json");
    let image_name = png_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "gui.png".to_string());
    let manifest = build_manifest(project, image_name, hover, tagged);
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| format!("encode json: {e}"))?;
    std::fs::write(&json_path, json).map_err(|e| format!("write json: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretch_is_one_full_region() {
        let r = layer_regions(LayerFit::Stretch, 16, 16, 10, 20, 100, 50, false, false);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].dst, [10.0, 20.0, 100.0, 50.0]);
        assert_eq!(r[0].uv, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn nine_slice_keeps_corners_native_and_covers_rect() {
        let regs = layer_regions(
            LayerFit::NineSlice {
                l: 4,
                r: 4,
                t: 4,
                b: 4,
            },
            16,
            16,
            0,
            0,
            80,
            60,
            false,
            false,
        );
        assert_eq!(regs.len(), 9);
        // Top-left corner: native inset size, sampling the source corner.
        let tl = regs
            .iter()
            .find(|r| r.dst[0] == 0.0 && r.dst[1] == 0.0)
            .unwrap();
        assert_eq!((tl.dst[2], tl.dst[3]), (4.0, 4.0));
        assert_eq!(tl.uv, [0.0, 0.0, 4.0 / 16.0, 4.0 / 16.0]);
        // The union of regions spans exactly the destination rect.
        let max_x = regs
            .iter()
            .map(|r| r.dst[0] + r.dst[2])
            .fold(0.0_f32, f32::max);
        let max_y = regs
            .iter()
            .map(|r| r.dst[1] + r.dst[3])
            .fold(0.0_f32, f32::max);
        assert_eq!((max_x, max_y), (80.0, 60.0));
    }

    #[test]
    fn nine_slice_insets_clamp_when_rect_too_small() {
        let regs = layer_regions(
            LayerFit::NineSlice {
                l: 8,
                r: 8,
                t: 8,
                b: 8,
            },
            16,
            16,
            0,
            0,
            6,
            6,
            false,
            false,
        );
        for r in &regs {
            assert!(r.dst[2] >= 0.0 && r.dst[3] >= 0.0);
        }
        let max_x = regs
            .iter()
            .map(|r| r.dst[0] + r.dst[2])
            .fold(0.0_f32, f32::max);
        assert!((max_x - 6.0).abs() < 0.001);
    }

    #[test]
    fn tile_covers_with_expected_piece_count() {
        // 32x16 source tiled over a 70x20 rect -> ceil(70/32)=3 cols, ceil(20/16)=2 rows.
        let regs = layer_regions(LayerFit::Tile, 32, 16, 0, 0, 70, 20, false, false);
        assert_eq!(regs.len(), 6);
        assert!(regs.iter().any(|r| r.uv[2] < 1.0));
        assert!(regs.iter().any(|r| r.uv[3] < 1.0));
    }

    #[test]
    fn flip_h_reverses_uv_keeps_dst_for_stretch() {
        let r = layer_regions(LayerFit::Stretch, 16, 16, 10, 20, 100, 50, true, false);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].dst, [10.0, 20.0, 100.0, 50.0]);
        assert_eq!((r[0].uv[0], r[0].uv[2]), (1.0, 0.0));
    }

    #[test]
    fn flip_h_mirrors_nine_slice_corner_to_far_side() {
        // The top-left 4x4 corner should land at the top-right after h-flip.
        let regs = layer_regions(
            LayerFit::NineSlice {
                l: 4,
                r: 4,
                t: 4,
                b: 4,
            },
            16,
            16,
            0,
            0,
            80,
            60,
            true,
            false,
        );
        assert!(regs.iter().any(|r| r.dst == [76.0, 0.0, 4.0, 4.0]));
    }

    #[test]
    fn rotate_90_maps_left_pixel_to_top() {
        // 2x1: left red, right blue. +90° CW -> 1x2 with red on top.
        let mut img = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 255]));
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, Rgba([0, 0, 255, 255]));
        let rot = rotate_rgba(&img, std::f32::consts::FRAC_PI_2);
        assert_eq!((rot.width(), rot.height()), (1, 2));
        assert_eq!(rot.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(rot.get_pixel(0, 1).0, [0, 0, 255, 255]);
    }
}
