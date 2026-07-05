//! The theme kit: one atlas of 9-sliceable parts with per-state faces, a
//! palette, widget metrics, and the font atlas.
//!
//! A part is looked up by key (`button.danger`, `panel.large`…); widgets pick
//! a *face* by state name (`default`, `hover`, `pressed`, `disabled`, `on`,
//! `off`, `selected`, `focus`, `empty`, `full`) with fallback to `default`.
//! Node `style` overrides the widget's default part key, so re-skinning is
//! data-only. The placeholder theme synthesizes flat programmer art for every
//! part so documents render (and tests run) before the real kit exists.

use crate::doc::{AlertLevel, Node, NodeKind};
use crate::layout::{LayoutEnv, SlotMetrics};
use crate::validate::StyleLookup;
use serde::Deserialize;
use std::collections::BTreeMap;

/// A CPU RGBA image the host uploads once (theme atlas, font atlas).
#[derive(Clone, Debug)]
pub struct ImageData {
    pub rgba: Vec<u8>,
    pub size: (u32, u32),
}

/// One drawable face of a part: an atlas pixel rect plus optional 9-slice
/// insets `[l, t, r, b]` (1x-art px = logical px).
#[derive(Clone, Debug, PartialEq)]
pub struct PartFace {
    pub rect: [u32; 4],
    pub slice: Option<[i32; 4]>,
}

/// A themed part: state-keyed faces plus label styling.
#[derive(Clone, Debug, Default)]
pub struct Part {
    faces: BTreeMap<String, PartFace>,
    pub label_color: Option<String>,
    /// Logical px the label shifts while pressed (classic push-in).
    pub pressed_label_offset: [i32; 2],
}

impl Part {
    /// The face for `state`, falling back to `default`, then to the part's
    /// first face (state-only parts like checkbox have `off`/`on` but no
    /// `default`).
    pub fn face(&self, state: &str) -> Option<&PartFace> {
        self.faces
            .get(state)
            .or_else(|| self.faces.get("default"))
            .or_else(|| self.faces.values().next())
    }

    /// The face for exactly `state` — no fallback (overlay faces like a
    /// slot's `hover`/`selected` highlight, drawn only when present).
    pub fn face_if(&self, state: &str) -> Option<&PartFace> {
        self.faces.get(state)
    }

    /// Natural (w, h) of the resting face — the part's authored pixel size.
    pub fn natural(&self) -> (i32, i32) {
        match self.face("default") {
            Some(f) => (f.rect[2] as i32, f.rect[3] as i32),
            None => (0, 0),
        }
    }
}

/// Layout-facing metrics with kit-tuned defaults.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub button_h: i32,
    /// Horizontal padding inside a button around its label.
    pub button_pad: i32,
    pub row_h: i32,
    pub slot: i32,
    pub slot_gap: i32,
    pub scrollbar_w: i32,
    /// Natural width of a slider track / text input when not sized by layout.
    pub slider_w: i32,
    pub input_w: i32,
    pub badge_pad: i32,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            button_h: 20,
            button_pad: 6,
            row_h: 26,
            slot: 18,
            slot_gap: 0,
            scrollbar_w: 8,
            slider_w: 64,
            input_w: 64,
            badge_pad: 3,
        }
    }
}

#[derive(Debug)]
pub struct ThemeError(pub String);

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ThemeError {}

pub struct Theme {
    palette: BTreeMap<String, [f32; 4]>,
    parts: BTreeMap<String, Part>,
    pub metrics: Metrics,
    pub atlas: ImageData,
    pub font: ImageData,
}

// ---- theme JSON --------------------------------------------------------------

#[derive(Deserialize)]
struct ThemeJson {
    format: u32,
    #[serde(default)]
    palette: BTreeMap<String, String>,
    atlas: String,
    #[serde(default)]
    font: Option<String>,
    parts: BTreeMap<String, PartJson>,
    #[serde(default)]
    metrics: Metrics,
}

#[derive(Deserialize)]
struct PartFaceJson {
    rect: [u32; 4],
    #[serde(default)]
    slice: Option<[i32; 4]>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PartJson {
    /// Shorthand: one stateless face.
    Single(PartFaceJson),
    Multi {
        states: BTreeMap<String, PartFaceJson>,
        #[serde(default)]
        label_color: Option<String>,
        #[serde(default)]
        pressed_label_offset: [i32; 2],
    },
}

impl Theme {
    /// Parse a theme manifest; `read` resolves image paths named by the
    /// manifest (relative to it) to file bytes.
    pub fn load(json: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Theme, ThemeError> {
        let t: ThemeJson =
            serde_json::from_str(json).map_err(|e| ThemeError(format!("theme manifest: {e}")))?;
        if t.format != 1 {
            return Err(ThemeError(format!("unsupported theme format {}", t.format)));
        }
        let mut palette = BTreeMap::new();
        for (k, v) in t.palette {
            palette.insert(k.clone(), parse_hex(&v).ok_or_else(|| {
                ThemeError(format!("palette '{k}': bad color '{v}' (want #RRGGBB[AA])"))
            })?);
        }
        let mut parts = BTreeMap::new();
        for (key, pj) in t.parts {
            let part = match pj {
                PartJson::Single(f) => {
                    let mut faces = BTreeMap::new();
                    faces.insert(
                        "default".to_owned(),
                        PartFace {
                            rect: f.rect,
                            slice: f.slice,
                        },
                    );
                    Part {
                        faces,
                        label_color: None,
                        pressed_label_offset: [0, 0],
                    }
                }
                PartJson::Multi {
                    states,
                    label_color,
                    pressed_label_offset,
                } => Part {
                    faces: states
                        .into_iter()
                        .map(|(s, f)| {
                            (
                                s,
                                PartFace {
                                    rect: f.rect,
                                    slice: f.slice,
                                },
                            )
                        })
                        .collect(),
                    label_color,
                    pressed_label_offset,
                },
            };
            if part.face("default").is_none() {
                return Err(ThemeError(format!("part '{key}' has no faces")));
            }
            parts.insert(key, part);
        }
        let atlas = load_png(&t.atlas, read)?;
        let font = match &t.font {
            Some(path) => load_png(path, read)?,
            None => builtin_font(),
        };
        Ok(Theme {
            palette,
            parts,
            metrics: t.metrics,
            atlas,
            font,
        })
    }

    pub fn part(&self, key: &str) -> Option<&Part> {
        self.parts.get(key)
    }

    /// Every part key in the kit, sorted (editor style pickers).
    pub fn style_keys(&self) -> impl Iterator<Item = &str> {
        self.parts.keys().map(String::as_str)
    }

    /// A styled container's chrome insets (its default face's 9-slice) —
    /// content and scrollbars sit inside them (border-box).
    pub fn container_insets(&self, node: &Node) -> [i32; 4] {
        if !node.kind.is_container() {
            return [0; 4];
        }
        self.part_for(node)
            .and_then(|p| p.face("default"))
            .and_then(|f| f.slice)
            .unwrap_or([0; 4])
    }

    /// A palette color by key; `#RRGGBB[AA]` literals pass through. Unknown
    /// keys are loud magenta rather than an error, so a missing palette entry
    /// is visible instead of fatal.
    pub fn color(&self, key: &str) -> [f32; 4] {
        if let Some(c) = self.palette.get(key) {
            return *c;
        }
        parse_hex(key).unwrap_or([1.0, 0.0, 1.0, 1.0])
    }

    /// The effective part for a node: its `style` override, else the widget
    /// default key.
    pub fn part_for(&self, node: &Node) -> Option<&Part> {
        match &node.style {
            Some(key) => self.parts.get(key),
            None => default_style_key(&node.kind).and_then(|k| self.parts.get(k)),
        }
    }

    /// Every texture the host should upload, with its paint id.
    pub fn textures(&self) -> [(crate::paint::TexId, &ImageData); 2] {
        [
            (crate::paint::TexId::ThemeAtlas, &self.atlas),
            (crate::paint::TexId::Font, &self.font),
        ]
    }
}

impl StyleLookup for Theme {
    fn has_style(&self, key: &str) -> bool {
        self.parts.contains_key(key)
    }
}

/// The default part key per widget type (node `style` overrides it).
pub fn default_style_key(kind: &NodeKind) -> Option<&'static str> {
    Some(match kind {
        NodeKind::Button { .. } => "button.default",
        NodeKind::Checkbox => "checkbox",
        NodeKind::Toggle => "toggle",
        NodeKind::Slider { .. } => "slider.track",
        NodeKind::TextInput { .. } => "input",
        NodeKind::Slot { .. } | NodeKind::SlotGrid { .. } => "slot",
        NodeKind::Badge { .. } => "badge",
        NodeKind::Alert { level, .. } => match level {
            AlertLevel::Info => "alert.info",
            AlertLevel::Warning => "alert.warning",
            AlertLevel::Success => "alert.success",
            AlertLevel::Danger => "alert.danger",
        },
        NodeKind::Label { .. } => "label",
        _ => return None,
    })
}

fn parse_hex(s: &str) -> Option<[f32; 4]> {
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

fn builtin_font() -> ImageData {
    let (rgba, size) = crate::text::build_atlas();
    ImageData { rgba, size }
}

// ---- layout env ---------------------------------------------------------------

/// The solver's window into the theme + the host's document-image registry
/// (image natural sizes live outside the theme).
pub struct ThemeEnv<'a> {
    pub theme: &'a Theme,
    pub image_size: &'a dyn Fn(&str) -> Option<(i32, i32)>,
}

impl LayoutEnv for ThemeEnv<'_> {
    fn leaf_size(
        &self,
        node: &Node,
        text: Option<&str>,
        image: Option<&str>,
        avail_w: Option<i32>,
    ) -> (i32, i32) {
        let m = &self.theme.metrics;
        let part_natural = |fallback: (i32, i32)| {
            self.theme
                .part_for(node)
                .map(|p| p.natural())
                .filter(|&(w, h)| w > 0 && h > 0)
                .unwrap_or(fallback)
        };
        match &node.kind {
            NodeKind::Label { wrap, scale, .. } => {
                let text = text.unwrap_or("");
                if *scale > 1 {
                    let (w, h) = crate::text::measure(text, None);
                    (w * *scale as i32, h * *scale as i32)
                } else {
                    crate::text::measure(text, if *wrap { avail_w } else { None })
                }
            }
            NodeKind::Button { icon, .. } => {
                let icon_w = icon
                    .as_deref()
                    .and_then(|k| self.theme.part(k))
                    .map(|p| p.natural().0)
                    .unwrap_or(0);
                let text_w = crate::text::width(text.unwrap_or(""));
                let gap = if icon_w > 0 && text_w > 0 { 4 } else { 0 };
                (icon_w + gap + text_w + m.button_pad * 2, m.button_h)
            }
            NodeKind::Checkbox => part_natural((10, 10)),
            NodeKind::Toggle => part_natural((18, 10)),
            NodeKind::Slider { .. } => {
                let handle_h = self
                    .theme
                    .part("slider.handle")
                    .map(|p| p.natural().1)
                    .unwrap_or(0);
                let track_h = part_natural((m.slider_w, 6)).1;
                (m.slider_w, handle_h.max(track_h))
            }
            NodeKind::TextInput { .. } => (m.input_w, part_natural((m.input_w, m.button_h)).1),
            NodeKind::Slot { .. } => (m.slot, m.slot),
            NodeKind::SlotGrid { cols, rows, .. } => {
                let (c, r) = (*cols as i32, *rows as i32);
                (
                    c * m.slot + (c - 1).max(0) * m.slot_gap,
                    r * m.slot + (r - 1).max(0) * m.slot_gap,
                )
            }
            NodeKind::Gauge { .. } => part_natural((0, 0)),
            NodeKind::Image { .. } | NodeKind::Rotimage { .. } => image
                .and_then(|name| (self.image_size)(name))
                .unwrap_or((0, 0)),
            NodeKind::Badge { .. } => {
                let text_w = crate::text::width(text.unwrap_or(""));
                let h = part_natural((0, crate::text::GLYPH_H + m.badge_pad * 2)).1;
                (text_w + m.badge_pad * 2, h)
            }
            NodeKind::Alert { .. } => {
                // Icon cell + text inside the frame insets; the text wraps
                // whenever the available width constrains it (an alert that
                // overflows its own frame is never right).
                let insets = self
                    .theme
                    .part_for(node)
                    .and_then(|p| p.face("default"))
                    .and_then(|f| f.slice)
                    .unwrap_or([4, 4, 4, 4]);
                let icon = self
                    .theme
                    .part(&format!(
                        "{}.icon",
                        node.style.as_deref().unwrap_or_else(|| {
                            default_style_key(&node.kind).unwrap_or("alert.info")
                        })
                    ))
                    .map(|p| p.natural())
                    .unwrap_or((0, 0));
                let gap = if icon.0 > 0 { 4 } else { 0 };
                let chrome_w = insets[0] + icon.0 + gap + insets[2];
                let text_avail = avail_w.map(|a| (a - chrome_w).max(crate::text::GLYPH_W));
                let (text_w, text_h) = crate::text::measure(text.unwrap_or(""), text_avail);
                (
                    chrome_w + text_w,
                    insets[1] + icon.1.max(text_h) + insets[3],
                )
            }
            _ => (0, 0),
        }
    }

    fn slot_metrics(&self) -> SlotMetrics {
        SlotMetrics {
            slot: self.theme.metrics.slot,
            gap: self.theme.metrics.slot_gap,
        }
    }

    fn container_insets(&self, node: &Node) -> [i32; 4] {
        self.theme.container_insets(node)
    }

    fn scrollbar_width(&self) -> i32 {
        self.theme.metrics.scrollbar_w
    }
}

// ---- placeholder theme ---------------------------------------------------------

impl Theme {
    /// A synthesized programmer-art theme covering every default part key:
    /// flat fills with 2px borders, distinct hues per state. Lets documents
    /// render and tests run before the real kit exists; replaced visually by
    /// the shipped `assets/ui/theme/`.
    pub fn placeholder() -> Theme {
        let mut atlas = PlaceholderAtlas::new(256, 256);
        let mut parts: BTreeMap<String, Part> = BTreeMap::new();

        let base_border = [90, 100, 110, 255];
        let single = |atlas: &mut PlaceholderAtlas,
                          parts: &mut BTreeMap<String, Part>,
                          key: &str,
                          w: u32,
                          h: u32,
                          fill: [u8; 4],
                          slice: Option<[i32; 4]>| {
            let rect = atlas.cell(w, h, fill, base_border);
            let mut faces = BTreeMap::new();
            faces.insert("default".to_owned(), PartFace { rect, slice });
            parts.insert(
                key.to_owned(),
                Part {
                    faces,
                    label_color: None,
                    pressed_label_offset: [0, 0],
                },
            );
        };
        let multi = |atlas: &mut PlaceholderAtlas,
                         parts: &mut BTreeMap<String, Part>,
                         key: &str,
                         w: u32,
                         h: u32,
                         slice: Option<[i32; 4]>,
                         states: &[(&str, [u8; 4])]| {
            let mut faces = BTreeMap::new();
            for (state, fill) in states {
                let rect = atlas.cell(w, h, *fill, base_border);
                faces.insert((*state).to_owned(), PartFace { rect, slice });
            }
            parts.insert(
                key.to_owned(),
                Part {
                    faces,
                    label_color: Some("text".to_owned()),
                    pressed_label_offset: [0, 1],
                },
            );
        };

        let sl4 = Some([4, 4, 4, 4]);
        single(&mut atlas, &mut parts, "panel.large", 32, 32, [24, 32, 40, 255], sl4);
        single(&mut atlas, &mut parts, "panel.inset", 16, 16, [16, 22, 28, 255], sl4);
        single(&mut atlas, &mut parts, "section.titled", 32, 32, [28, 36, 44, 255], Some([4, 12, 4, 4]));
        multi(&mut atlas, &mut parts, "button.default", 24, 20, sl4, &[
            ("default", [45, 58, 70, 255]),
            ("hover", [62, 80, 96, 255]),
            ("pressed", [35, 45, 55, 255]),
            ("disabled", [38, 42, 46, 255]),
        ]);
        multi(&mut atlas, &mut parts, "button.success", 24, 20, sl4, &[
            ("default", [40, 90, 45, 255]),
            ("hover", [55, 115, 60, 255]),
            ("pressed", [30, 70, 35, 255]),
            ("disabled", [40, 52, 42, 255]),
        ]);
        multi(&mut atlas, &mut parts, "button.danger", 24, 20, sl4, &[
            ("default", [110, 40, 40, 255]),
            ("hover", [140, 55, 55, 255]),
            ("pressed", [85, 30, 30, 255]),
            ("disabled", [56, 40, 40, 255]),
        ]);
        multi(&mut atlas, &mut parts, "checkbox", 10, 10, None, &[
            ("off", [30, 38, 46, 255]),
            ("on", [80, 190, 90, 255]),
            ("disabled", [40, 44, 48, 255]),
        ]);
        multi(&mut atlas, &mut parts, "toggle", 18, 10, None, &[
            ("off", [55, 60, 66, 255]),
            ("on", [70, 170, 80, 255]),
            ("disabled", [42, 46, 50, 255]),
        ]);
        multi(&mut atlas, &mut parts, "slot", 18, 18, Some([1, 1, 1, 1]), &[
            ("default", [20, 26, 32, 255]),
            ("hover", [90, 110, 130, 160]),
        ]);
        single(&mut atlas, &mut parts, "scrollbar.track", 8, 24, [18, 24, 30, 255], Some([2, 2, 2, 2]));
        multi(&mut atlas, &mut parts, "scrollbar.thumb", 8, 16, Some([2, 2, 2, 2]), &[
            ("default", [90, 100, 110, 255]),
            ("hover", [120, 132, 144, 255]),
        ]);
        single(&mut atlas, &mut parts, "slider.track", 24, 6, [30, 60, 90, 255], Some([2, 2, 2, 2]));
        multi(&mut atlas, &mut parts, "slider.handle", 8, 14, None, &[
            ("default", [150, 160, 170, 255]),
            ("hover", [190, 200, 210, 255]),
            ("pressed", [120, 130, 140, 255]),
            ("disabled", [80, 84, 88, 255]),
        ]);
        multi(&mut atlas, &mut parts, "list.row", 32, 26, sl4, &[
            ("default", [26, 34, 42, 255]),
            ("hover", [38, 50, 62, 255]),
            ("selected", [50, 70, 100, 255]),
        ]);
        multi(&mut atlas, &mut parts, "input", 32, 18, sl4, &[
            ("default", [16, 20, 24, 255]),
            ("focus", [22, 30, 40, 255]),
            ("disabled", [30, 32, 34, 255]),
        ]);
        single(&mut atlas, &mut parts, "badge", 16, 13, [40, 52, 64, 255], Some([2, 2, 2, 2]));
        for (level, fill) in [
            ("info", [30, 60, 110, 255]),
            ("warning", [110, 90, 20, 255]),
            ("success", [30, 90, 40, 255]),
            ("danger", [110, 35, 35, 255]),
        ] {
            single(&mut atlas, &mut parts, &format!("alert.{level}"), 32, 20, fill, sl4);
        }
        multi(&mut atlas, &mut parts, "gauge.arrow", 24, 17, None, &[
            ("empty", [40, 44, 48, 255]),
            ("full", [230, 230, 230, 255]),
        ]);
        multi(&mut atlas, &mut parts, "gauge.flame", 14, 14, None, &[
            ("empty", [40, 40, 40, 255]),
            ("full", [230, 140, 40, 255]),
        ]);
        single(&mut atlas, &mut parts, "label", 1, 1, [0, 0, 0, 0], None);

        let mut palette = BTreeMap::new();
        for (k, v) in [
            ("text", "#E8EDF2"),
            ("text_muted", "#9AA7B4"),
            ("text_disabled", "#5E6B78"),
            ("accent", "#57C956"),
            ("danger", "#E4574F"),
            ("selection", "#3E6FD9"),
            ("dim", "#00000080"),
        ] {
            palette.insert(k.to_owned(), parse_hex(v).unwrap());
        }

        Theme {
            palette,
            parts,
            metrics: Metrics::default(),
            atlas: atlas.finish(),
            font: builtin_font(),
        }
    }
}

/// Shelf-packs flat-colored bordered cells into an RGBA atlas.
struct PlaceholderAtlas {
    rgba: Vec<u8>,
    size: (u32, u32),
    cursor: (u32, u32),
    row_h: u32,
}

impl PlaceholderAtlas {
    fn new(w: u32, h: u32) -> PlaceholderAtlas {
        PlaceholderAtlas {
            rgba: vec![0; (w * h * 4) as usize],
            size: (w, h),
            cursor: (0, 0),
            row_h: 0,
        }
    }

    fn cell(&mut self, w: u32, h: u32, fill: [u8; 4], border: [u8; 4]) -> [u32; 4] {
        if self.cursor.0 + w > self.size.0 {
            self.cursor = (0, self.cursor.1 + self.row_h + 1);
            self.row_h = 0;
        }
        assert!(
            self.cursor.1 + h <= self.size.1,
            "placeholder atlas overflow"
        );
        let (x0, y0) = self.cursor;
        for y in 0..h {
            for x in 0..w {
                let on_border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                let c = if on_border { border } else { fill };
                let i = (((y0 + y) * self.size.0 + x0 + x) * 4) as usize;
                self.rgba[i..i + 4].copy_from_slice(&c);
            }
        }
        self.cursor.0 += w + 1;
        self.row_h = self.row_h.max(h);
        [x0, y0, w, h]
    }

    fn finish(self) -> ImageData {
        ImageData {
            rgba: self.rgba,
            size: self.size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;

    #[test]
    fn placeholder_theme_has_every_default_part() {
        let t = Theme::placeholder();
        for key in [
            "panel.large",
            "button.default",
            "button.danger",
            "checkbox",
            "toggle",
            "slot",
            "scrollbar.thumb",
            "slider.track",
            "slider.handle",
            "list.row",
            "input",
            "badge",
            "alert.info",
        ] {
            assert!(t.part(key).is_some(), "missing part '{key}'");
        }
        assert!(t.part("checkbox").unwrap().face("on").is_some());
        assert!(
            t.part("checkbox").unwrap().face("bogus_state").is_some(),
            "unknown states fall back to a face"
        );
        let (aw, ah) = t.atlas.size;
        assert_eq!(t.atlas.rgba.len(), (aw * ah * 4) as usize);
    }

    #[test]
    fn theme_json_parses_shorthand_and_state_parts() {
        let json = r##"{
            "format": 1,
            "palette": { "text": "#E8EDF2", "accent": "#57C95680" },
            "atlas": "kit.png",
            "parts": {
                "panel.large": { "rect": [0,0,64,64], "slice": [8,8,8,8] },
                "button.default": {
                    "states": {
                        "default": { "rect": [0,64,32,20], "slice": [4,4,4,4] },
                        "hover":   { "rect": [32,64,32,20], "slice": [4,4,4,4] }
                    },
                    "label_color": "text",
                    "pressed_label_offset": [0, 1]
                }
            },
            "metrics": { "slot": 20 }
        }"##;
        // 1x1 transparent png.
        let png = {
            let img = image::RgbaImage::new(4, 4);
            let mut bytes = std::io::Cursor::new(Vec::new());
            img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            bytes.into_inner()
        };
        let t = Theme::load(json, &|p| (p == "kit.png").then(|| png.clone())).unwrap();
        assert_eq!(t.part("panel.large").unwrap().natural(), (64, 64));
        let b = t.part("button.default").unwrap();
        assert_eq!(b.face("hover").unwrap().rect, [32, 64, 32, 20]);
        assert_eq!(b.face("pressed").unwrap().rect, [0, 64, 32, 20], "fallback to default");
        assert_eq!(b.pressed_label_offset, [0, 1]);
        assert_eq!(t.metrics.slot, 20);
        assert_eq!(t.color("accent")[3], 128.0 / 255.0);
        assert_eq!(t.color("#FF0000"), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.color("nope"), [1.0, 0.0, 1.0, 1.0], "missing key is loud magenta");
        // Font defaults to the builtin atlas.
        assert_eq!(t.font.size, crate::text::atlas_size());
    }

    #[test]
    fn alerts_wrap_when_width_constrained() {
        let t = Theme::placeholder();
        let env = ThemeEnv {
            theme: &t,
            image_size: &|_| None,
        };
        let doc = Document::from_json(
            r#"{
            "format": 1, "kind": "llama:x", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "alert", "level": "info", "text": "A rather long warning message" }
            ] }
        }"#,
        )
        .unwrap();
        let alert = &doc.root.children[0];
        let text = Some("A rather long warning message");
        let (w_free, h_free) = env.leaf_size(alert, text, None, None);
        let (w_tight, h_tight) = env.leaf_size(alert, text, None, Some(100));
        assert!(w_tight <= 100, "constrained alert fits its width: {w_tight}");
        assert!(w_tight < w_free);
        assert!(h_tight > h_free, "wrapped alert grows taller instead of overflowing");
    }

    #[test]
    fn theme_env_supplies_widget_naturals() {
        let t = Theme::placeholder();
        let env = ThemeEnv {
            theme: &t,
            image_size: &|name| (name == "wheel.png").then_some((32, 32)),
        };
        let doc = Document::from_json(
            r#"{
            "format": 1, "kind": "llama:x", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "button", "id": "b", "text": "OK" },
                { "type": "checkbox", "id": "c" },
                { "type": "slot_grid", "role": "hotbar", "cols": 9, "rows": 1 },
                { "type": "image", "image": "wheel.png" },
                { "type": "image", "image": "missing.png" }
            ] }
        }"#,
        )
        .unwrap();
        let n = &doc.root.children;
        assert_eq!(
            env.leaf_size(&n[0], Some("OK"), None, None),
            (crate::text::width("OK") + 12, 20)
        );
        assert_eq!(env.leaf_size(&n[1], None, None, None), (10, 10));
        assert_eq!(env.leaf_size(&n[2], None, None, None), (18 * 9, 18));
        assert_eq!(
            env.leaf_size(&n[3], None, Some("wheel.png"), None),
            (32, 32)
        );
        assert_eq!(env.leaf_size(&n[4], None, Some("missing.png"), None), (0, 0));
    }
}
