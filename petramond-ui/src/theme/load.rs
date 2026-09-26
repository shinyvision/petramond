//! Theme manifests and the layered stack they load into.
//!
//! Every manifest struct denies unknown fields: a misspelt key in a pack's
//! theme is an error naming the key, never a silently ignored property.

use super::{palette, FaceState, ImageData, Metrics, Part, PartFace, Theme, ThemeError};
use serde::Deserialize;
use std::collections::BTreeMap;

/// One manifest of a theme stack, with the reader that resolves the image
/// and font paths it names (relative to that manifest).
#[derive(Clone, Copy)]
pub struct ThemeLayer<'a> {
    pub json: &'a str,
    pub read: &'a dyn Fn(&str) -> Option<Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeJson {
    format: u32,
    #[serde(default)]
    palette: BTreeMap<String, String>,
    /// This layer's atlas page; required when the layer defines parts.
    #[serde(default)]
    atlas: Option<String>,
    #[serde(default)]
    font: Option<FontJson>,
    #[serde(default)]
    parts: BTreeMap<String, PartJson>,
    /// Merged key by key over the layers below (then checked as
    /// [`Metrics`], so a misspelt metric is an error too).
    #[serde(default)]
    metrics: serde_json::Map<String, serde_json::Value>,
}

/// The theme's font: a real font FILE plus the pixel size it was designed
/// for (a pixel font rasterized off its design grid loses whole stems, so the
/// size is authored, never guessed), optionally limited to inclusive
/// codepoint `ranges`, and followed by `fallback` faces that fill whatever
/// the primary lacks — a pack can add a script without replacing the face.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FontJson {
    file: String,
    px: f32,
    #[serde(default)]
    ranges: Vec<[u32; 2]>,
    #[serde(default)]
    fallback: Vec<FaceJson>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FaceJson {
    file: String,
    px: f32,
    #[serde(default)]
    ranges: Vec<[u32; 2]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartFaceJson {
    rect: [u32; 4],
    #[serde(default)]
    slice: Option<[i32; 4]>,
}

/// A part: either one stateless face (`rect` + optional `slice`) or
/// state-keyed `states`, plus optional label styling.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartJson {
    #[serde(default)]
    rect: Option<[u32; 4]>,
    #[serde(default)]
    slice: Option<[i32; 4]>,
    #[serde(default)]
    states: Option<BTreeMap<String, PartFaceJson>>,
    #[serde(default)]
    label_color: Option<String>,
    #[serde(default)]
    pressed_label_offset: [i32; 2],
}

impl PartJson {
    fn into_part(self, key: &str, page: u16) -> Result<Part, ThemeError> {
        let faces = match (self.rect, self.states) {
            (Some(rect), None) => BTreeMap::from([(
                FaceState::Default,
                PartFace {
                    page,
                    rect,
                    slice: self.slice,
                },
            )]),
            (None, Some(states)) => {
                if self.slice.is_some() {
                    return Err(ThemeError(format!(
                        "part '{key}': `slice` belongs on each state's face"
                    )));
                }
                let mut faces = BTreeMap::new();
                for (name, face) in states {
                    let state = FaceState::parse(&name).ok_or_else(|| {
                        ThemeError(format!("part '{key}': unknown face state '{name}'"))
                    })?;
                    faces.insert(
                        state,
                        PartFace {
                            page,
                            rect: face.rect,
                            slice: face.slice,
                        },
                    );
                }
                faces
            }
            (Some(_), Some(_)) => {
                return Err(ThemeError(format!(
                    "part '{key}': give either `rect` or `states`, not both"
                )))
            }
            (None, None) => {
                return Err(ThemeError(format!("part '{key}' has no faces")));
            }
        };
        if faces.is_empty() {
            return Err(ThemeError(format!("part '{key}' has no faces")));
        }
        Ok(Part {
            faces,
            label_color: self.label_color,
            pressed_label_offset: self.pressed_label_offset,
        })
    }
}

impl Theme {
    /// Parse one theme manifest; `read` resolves image paths named by the
    /// manifest (relative to it) to file bytes.
    pub fn load(json: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Theme, ThemeError> {
        Theme::load_stack(&[ThemeLayer { json, read }])
    }

    /// Load a theme stack, base first. Each later layer adds or replaces
    /// parts by key, palette entries by key and metrics by key; the topmost
    /// layer that declares a font supplies it. Every layer that defines parts
    /// brings its own atlas page. A layer error names the layer.
    pub fn load_stack(layers: &[ThemeLayer<'_>]) -> Result<Theme, ThemeError> {
        if layers.is_empty() {
            return Err(ThemeError("empty theme stack".into()));
        }
        let mut palette_hex: BTreeMap<String, [f32; 4]> = BTreeMap::new();
        let mut parts: BTreeMap<String, Part> = BTreeMap::new();
        let mut metrics = serde_json::Map::new();
        let mut pages: Vec<ImageData> = Vec::new();
        let mut font: Option<(FontJson, ThemeLayer<'_>)> = None;
        for (i, layer) in layers.iter().enumerate() {
            let at = |e: String| ThemeError(format!("theme layer {i}: {e}"));
            let t: ThemeJson =
                serde_json::from_str(layer.json).map_err(|e| at(format!("manifest: {e}")))?;
            if t.format != 1 {
                return Err(at(format!("unsupported theme format {}", t.format)));
            }
            for (k, v) in t.palette {
                let color = parse_hex(&v).ok_or_else(|| {
                    at(format!("palette '{k}': bad color '{v}' (want #RRGGBB[AA])"))
                })?;
                palette_hex.insert(k, color);
            }
            if !t.parts.is_empty() {
                let atlas = t
                    .atlas
                    .as_deref()
                    .ok_or_else(|| at("defines parts but names no `atlas`".into()))?;
                let page = u16::try_from(pages.len())
                    .map_err(|_| at("too many atlas pages".into()))?;
                pages.push(load_png(atlas, layer.read).map_err(|e| at(e.0))?);
                for (key, pj) in t.parts {
                    let part = pj.into_part(&key, page).map_err(|e| at(e.0))?;
                    parts.insert(key, part);
                }
            }
            metrics.extend(t.metrics);
            if let Some(f) = t.font {
                font = Some((f, *layer));
            }
        }
        let metrics: Metrics = serde_json::from_value(serde_json::Value::Object(metrics))
            .map_err(|e| ThemeError(format!("theme metrics: {e}")))?;
        for key in palette::REQUIRED {
            if !palette_hex.contains_key(key) {
                return Err(ThemeError(format!(
                    "palette has no '{key}' (widgets paint with it)"
                )));
            }
        }
        for (key, part) in &parts {
            if let Some(color) = part.label_color.as_deref() {
                if !palette_hex.contains_key(color) && parse_hex(color).is_none() {
                    return Err(ThemeError(format!(
                        "part '{key}': label_color '{color}' is not a palette key or #RRGGBB[AA]"
                    )));
                }
            }
        }
        let ui_font = match font {
            Some((f, layer)) => load_font(&f, layer.read)?,
            None => crate::text::Font::builtin(),
        };
        Ok(Theme {
            palette: palette_hex,
            parts,
            metrics,
            pages,
            ui_font: std::sync::Arc::new(ui_font),
        })
    }
}

pub(super) fn parse_hex(s: &str) -> Option<[f32; 4]> {
    let hex = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    let (r, g, b, a) = match hex.len() {
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ])
}

/// The manifest's face chain (primary, then fallbacks) as one font.
fn load_font(
    font: &FontJson,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<crate::text::Font, ThemeError> {
    let faces: Vec<(&str, f32, &[[u32; 2]])> =
        std::iter::once((font.file.as_str(), font.px, font.ranges.as_slice()))
            .chain(
                font.fallback
                    .iter()
                    .map(|f| (f.file.as_str(), f.px, f.ranges.as_slice())),
            )
            .collect();
    let bytes = faces
        .iter()
        .map(|(file, ..)| read(file).ok_or_else(|| ThemeError(format!("font '{file}' not found"))))
        .collect::<Result<Vec<_>, _>>()?;
    let sources: Vec<crate::text::FaceSource<'_>> = faces
        .iter()
        .zip(&bytes)
        .map(|(&(_, px, ranges), bytes)| crate::text::FaceSource { bytes, px, ranges })
        .collect();
    crate::text::Font::from_faces(&sources)
        .map_err(|e| ThemeError(format!("font '{}': {e}", font.file)))
}

fn load_png(path: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<ImageData, ThemeError> {
    let bytes = read(path).ok_or_else(|| ThemeError(format!("missing theme image '{path}'")))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| ThemeError(format!("'{path}': {e}")))?
        .to_rgba8();
    let size = img.dimensions();
    Ok(ImageData {
        rgba: img.into_raw(),
        size,
    })
}
