//! The theme kit: 9-sliceable parts with per-state faces on one or more
//! atlas pages, a palette, widget metrics, and the UI font.
//!
//! A part is looked up by key (`button.danger`, `panel.large`…); widgets pick
//! a *face* by [`FaceState`] with fallback to `default`. Node `style`
//! overrides the widget's default part key, so re-skinning is data-only.
//!
//! A theme is a STACK of manifests (see [`Theme::load_stack`]): the base kit
//! first, then pack overlays that add or replace parts by key, add palette
//! entries and metrics, and may bring their own font. Each layer's atlas is
//! its own page ([`crate::TexId::ThemePage`]), so several packs can each add
//! chrome without sharing one PNG. State names and palette references are
//! checked when the stack loads, not at paint time.
//!
//! The placeholder theme synthesizes flat programmer art for every part so
//! documents render (and tests run) before the real kit exists.

mod env;
mod load;
mod placeholder;

pub use env::ThemeEnv;
pub use load::ThemeLayer;

use crate::doc::{Node, NodeKind};
use crate::validate::StyleLookup;
use serde::Deserialize;
use std::collections::BTreeMap;

/// A CPU RGBA image the host uploads (theme atlas pages, the font atlas).
#[derive(Clone, Debug)]
pub struct ImageData {
    pub rgba: Vec<u8>,
    pub size: (u32, u32),
}

/// Every face state a widget can ask a part for. Manifests name them by
/// [`FaceState::name`]; an unknown name is a load error, not a silent miss.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FaceState {
    Default,
    Hover,
    Pressed,
    Disabled,
    Selected,
    Focus,
    Off,
    On,
    /// A checkbox/toggle's hovered or pressed look per position — for
    /// controls whose handle sits in a different place in each position.
    OffHover,
    OnHover,
    OffPressed,
    OnPressed,
    /// Gauge backgrounds and fills.
    Empty,
    Full,
}

impl FaceState {
    pub const ALL: [FaceState; 14] = [
        FaceState::Default,
        FaceState::Hover,
        FaceState::Pressed,
        FaceState::Disabled,
        FaceState::Selected,
        FaceState::Focus,
        FaceState::Off,
        FaceState::On,
        FaceState::OffHover,
        FaceState::OnHover,
        FaceState::OffPressed,
        FaceState::OnPressed,
        FaceState::Empty,
        FaceState::Full,
    ];

    /// The manifest spelling.
    pub fn name(self) -> &'static str {
        match self {
            FaceState::Default => "default",
            FaceState::Hover => "hover",
            FaceState::Pressed => "pressed",
            FaceState::Disabled => "disabled",
            FaceState::Selected => "selected",
            FaceState::Focus => "focus",
            FaceState::Off => "off",
            FaceState::On => "on",
            FaceState::OffHover => "off.hover",
            FaceState::OnHover => "on.hover",
            FaceState::OffPressed => "off.pressed",
            FaceState::OnPressed => "on.pressed",
            FaceState::Empty => "empty",
            FaceState::Full => "full",
        }
    }

    pub fn parse(name: &str) -> Option<FaceState> {
        FaceState::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// Palette entries the widgets themselves paint with. Every theme stack must
/// define them (checked at load).
pub mod palette {
    pub const TEXT: &str = "text";
    pub const TEXT_MUTED: &str = "text_muted";
    pub const TEXT_DISABLED: &str = "text_disabled";
    pub const SELECTION: &str = "selection";
    pub const REQUIRED: [&str; 4] = [TEXT, TEXT_MUTED, TEXT_DISABLED, SELECTION];
}

/// One drawable face of a part: an atlas page, a pixel rect on it, plus
/// optional 9-slice insets `[l, t, r, b]` (1x-art px = logical px).
#[derive(Clone, Debug, PartialEq)]
pub struct PartFace {
    pub page: u16,
    pub rect: [u32; 4],
    pub slice: Option<[i32; 4]>,
}

/// A themed part: state-keyed faces plus label styling.
#[derive(Clone, Debug, Default)]
pub struct Part {
    faces: BTreeMap<FaceState, PartFace>,
    /// Palette key the label paints with (validated at load).
    pub label_color: Option<String>,
    /// Logical px the label shifts while pressed (classic push-in).
    pub pressed_label_offset: [i32; 2],
}

impl Part {
    /// The face for `state`, falling back to `default`, then to the part's
    /// first face (state-only parts like checkbox have `off`/`on` but no
    /// `default`).
    pub fn face(&self, state: FaceState) -> Option<&PartFace> {
        self.faces
            .get(&state)
            .or_else(|| self.faces.get(&FaceState::Default))
            .or_else(|| self.faces.values().next())
    }

    /// The face for exactly `state` — no fallback (overlay faces like a
    /// slot's `hover`/`selected` highlight, drawn only when present).
    pub fn face_if(&self, state: FaceState) -> Option<&PartFace> {
        self.faces.get(&state)
    }

    /// Natural (w, h) of the resting face — the part's authored pixel size.
    pub fn natural(&self) -> (i32, i32) {
        match self.face(FaceState::Default) {
            Some(f) => (f.rect[2] as i32, f.rect[3] as i32),
            None => (0, 0),
        }
    }
}

/// Layout-facing metrics with kit-tuned defaults.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
    /// Tab-bar cell height and gap between tab cells.
    pub tab_h: i32,
    pub tab_gap: i32,
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
            tab_h: 20,
            tab_gap: 2,
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
    /// Atlas pages, one per stack layer that brings art; faces index them.
    pages: Vec<ImageData>,
    ui_font: std::sync::Arc<crate::text::Font>,
}

impl Theme {
    /// The theme's UI font — the one font its documents are measured, hit
    /// tested and painted with. Its glyph atlas grows as text first uses a
    /// glyph: hosts re-upload [`Theme::font_atlas`] whenever
    /// `ui_font().atlas_revision()` moves.
    pub fn ui_font(&self) -> &std::sync::Arc<crate::text::Font> {
        &self.ui_font
    }

    /// The font's glyph atlas as it stands now (see [`Theme::ui_font`]).
    pub fn font_atlas(&self) -> ImageData {
        let (rgba, size) = self.ui_font.build_atlas();
        ImageData { rgba, size }
    }

    /// Every atlas page, indexed by [`PartFace::page`] and
    /// [`crate::TexId::ThemePage`].
    pub fn pages(&self) -> &[ImageData] {
        &self.pages
    }

    /// The size of atlas page `page` (`(1, 1)` for a page that does not
    /// exist, so a stray index samples nothing rather than dividing by zero).
    pub fn page_size(&self, page: u16) -> (u32, u32) {
        self.pages.get(page as usize).map_or((1, 1), |p| p.size)
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
        if !node.lays_out_children() {
            return [0; 4];
        }
        self.part_for(node)
            .and_then(|p| p.face(FaceState::Default))
            .and_then(|f| f.slice)
            .unwrap_or([0; 4])
    }

    /// A palette color by key; `#RRGGBB[AA]` literals pass through. A key
    /// missing at paint time (a document's bound palette name) is loud
    /// magenta rather than an error, so the gap is visible instead of fatal;
    /// the keys widgets and parts name are checked at load.
    pub fn color(&self, key: &str) -> [f32; 4] {
        if let Some(c) = self.palette.get(key) {
            return *c;
        }
        load::parse_hex(key).unwrap_or([1.0, 0.0, 1.0, 1.0])
    }

    /// Whether `key` names a palette entry or is a colour literal.
    pub fn has_color(&self, key: &str) -> bool {
        self.palette.contains_key(key) || load::parse_hex(key).is_some()
    }

    /// The effective part for a node: its `style` override, else the widget
    /// default key.
    pub fn part_for(&self, node: &Node) -> Option<&Part> {
        match &node.style {
            Some(key) => self.parts.get(key),
            None => default_style_key(&node.kind).and_then(|k| self.parts.get(k)),
        }
    }
}

impl StyleLookup for Theme {
    fn has_style(&self, key: &str) -> bool {
        self.parts.contains_key(key)
    }
}

/// The default part key per widget type (node `style` overrides it) — this is
/// the public face of the crate-private `widget_policy::style_key` table,
/// which holds the exhaustive per-kind answer.
pub fn default_style_key(kind: &NodeKind) -> Option<&'static str> {
    crate::widget_policy::style_key(kind)
}

#[cfg(test)]
mod tests;
