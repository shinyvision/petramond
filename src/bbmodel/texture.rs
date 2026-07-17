use serde_json::Value;

use super::parse::{base64_decode, project_resolution};

/// One texture's slot in the combined sheet: its own UV divisor plus the
/// normalized offset/scale of its band, so a face UV remaps in one step.
pub(super) struct TexRect {
    uv_w: f32,
    uv_h: f32,
    u_off: f32,
    v_off: f32,
    u_scale: f32,
    v_scale: f32,
}

impl TexRect {
    /// A raw face UV coordinate (in this texture's own UV units) → the sheet.
    pub(super) fn remap(&self, u: f32, v: f32) -> (f32, f32) {
        (
            self.u_off + u / self.uv_w * self.u_scale,
            self.v_off + v / self.uv_h * self.v_scale,
        )
    }
}

/// Every embedded texture decoded and stacked vertically into one RGBA sheet.
/// Faces reference textures by array index; `rects` is index-aligned with the
/// authored `textures` array (`None` = that entry had no decodable source).
pub(super) struct TextureSheet {
    pub(super) rgba: Vec<u8>,
    pub(super) w: u32,
    pub(super) h: u32,
    rects: Vec<Option<TexRect>>,
}

impl TextureSheet {
    /// The rect for a face's texture reference: its own entry when it decoded,
    /// else the first decoded texture (the old first-texture-only behavior).
    pub(super) fn rect(&self, index: Option<usize>) -> Option<&TexRect> {
        index
            .and_then(|i| self.rects.get(i))
            .and_then(Option::as_ref)
            .or_else(|| self.rects.iter().flatten().next())
    }

    pub(super) fn decode(root: &Value) -> Result<TextureSheet, String> {
        let (res_w, res_h) = project_resolution(root);
        let empty = Vec::new();
        let texs = root
            .get("textures")
            .and_then(Value::as_array)
            .unwrap_or(&empty);

        // Decode each entry's `data:image/png;base64,<payload>` source; an entry
        // without one (or that fails to decode) stays `None` so indices keep lining
        // up with face references.
        struct DecodedTex {
            rgba: Vec<u8>,
            w: u32,
            h: u32,
            /// UV-space size the face coordinates are authored in (entry override
            /// or the project resolution).
            uv_w: f32,
            uv_h: f32,
        }
        let images: Vec<Option<DecodedTex>> = texs
            .iter()
            .map(|t| {
                let src = t.get("source").and_then(Value::as_str)?;
                let payload = src.split_once(',').map(|(_, b)| b).unwrap_or(src);
                let bytes = base64_decode(payload)?;
                let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
                let (w, h) = (img.width(), img.height());
                let uv_w = t.get("uv_width").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                let uv_h = t.get("uv_height").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                let (uv_w, uv_h) = if uv_w > 0.0 && uv_h > 0.0 {
                    (uv_w, uv_h)
                } else {
                    (res_w, res_h)
                };
                Some(DecodedTex {
                    rgba: img.into_raw(),
                    w,
                    h,
                    uv_w,
                    uv_h,
                })
            })
            .collect();

        let sheet_w = images.iter().flatten().map(|i| i.w).max().unwrap_or(0);
        let sheet_h: u32 = images.iter().flatten().map(|i| i.h).sum();
        if sheet_w == 0 || sheet_h == 0 {
            return Err("no embedded texture source".into());
        }

        let mut rgba = vec![0u8; (sheet_w * sheet_h * 4) as usize];
        let mut rects = Vec::with_capacity(images.len());
        let mut y_off = 0u32;
        for img in images {
            let Some(tex) = img else {
                rects.push(None);
                continue;
            };
            for row in 0..tex.h {
                let src = (row * tex.w * 4) as usize;
                let dst = (((y_off + row) * sheet_w) * 4) as usize;
                rgba[dst..dst + (tex.w * 4) as usize]
                    .copy_from_slice(&tex.rgba[src..src + (tex.w * 4) as usize]);
            }
            rects.push(Some(TexRect {
                uv_w: tex.uv_w,
                uv_h: tex.uv_h,
                u_off: 0.0,
                v_off: y_off as f32 / sheet_h as f32,
                u_scale: tex.w as f32 / sheet_w as f32,
                v_scale: tex.h as f32 / sheet_h as f32,
            }));
            y_off += tex.h;
        }

        Ok(TextureSheet {
            rgba,
            w: sheet_w,
            h: sheet_h,
            rects,
        })
    }
}
