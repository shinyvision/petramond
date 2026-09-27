mod env;
mod load;
mod placeholder;

pub use env::ThemeEnv;
pub use load::ThemeLayer;

use crate::doc::{Node, NodeKind};
use crate::validate::StyleLookup;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct ImageData {
    pub rgba: Vec<u8>,
    pub size: (u32, u32),
}

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
    OffHover,
    OnHover,
    OffPressed,
    OnPressed,
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

pub mod palette {
    pub const TEXT: &str = "text";
    pub const TEXT_MUTED: &str = "text_muted";
    pub const TEXT_DISABLED: &str = "text_disabled";
    pub const SELECTION: &str = "selection";
    pub const REQUIRED: [&str; 4] = [TEXT, TEXT_MUTED, TEXT_DISABLED, SELECTION];
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartFace {
    pub page: u16,
    pub rect: [u32; 4],
    pub slice: Option<[i32; 4]>,
}

#[derive(Clone, Debug, Default)]
pub struct Part {
    faces: BTreeMap<FaceState, PartFace>,
    pub label_color: Option<String>,
    pub pressed_label_offset: [i32; 2],
}

impl Part {
    pub fn face(&self, state: FaceState) -> Option<&PartFace> {
        self.faces
            .get(&state)
            .or_else(|| self.faces.get(&FaceState::Default))
            .or_else(|| self.faces.values().next())
    }

    pub fn face_if(&self, state: FaceState) -> Option<&PartFace> {
        self.faces.get(&state)
    }

    pub fn natural(&self) -> (i32, i32) {
        match self.face(FaceState::Default) {
            Some(f) => (f.rect[2] as i32, f.rect[3] as i32),
            None => (0, 0),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Metrics {
    pub button_h: i32,
    pub button_pad: i32,
    pub row_h: i32,
    pub slot: i32,
    pub slot_gap: i32,
    pub scrollbar_w: i32,
    pub slider_w: i32,
    pub input_w: i32,
    pub badge_pad: i32,
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
    pages: Vec<ImageData>,
    ui_font: std::sync::Arc<crate::text::Font>,
}

impl Theme {
    pub fn ui_font(&self) -> &std::sync::Arc<crate::text::Font> {
        &self.ui_font
    }

    pub fn font_atlas(&self) -> ImageData {
        let (rgba, size) = self.ui_font.build_atlas();
        ImageData { rgba, size }
    }

    pub fn pages(&self) -> &[ImageData] {
        &self.pages
    }

    pub fn page_size(&self, page: u16) -> (u32, u32) {
        self.pages.get(page as usize).map_or((1, 1), |p| p.size)
    }

    pub fn part(&self, key: &str) -> Option<&Part> {
        self.parts.get(key)
    }

    pub fn style_keys(&self) -> impl Iterator<Item = &str> {
        self.parts.keys().map(String::as_str)
    }

    pub fn container_insets(&self, node: &Node) -> [i32; 4] {
        if !node.lays_out_children() {
            return [0; 4];
        }
        self.part_for(node)
            .and_then(|p| p.face(FaceState::Default))
            .and_then(|f| f.slice)
            .unwrap_or([0; 4])
    }

    pub fn color(&self, key: &str) -> [f32; 4] {
        if let Some(c) = self.palette.get(key) {
            return *c;
        }
        load::parse_hex(key).unwrap_or([1.0, 0.0, 1.0, 1.0])
    }

    pub fn has_color(&self, key: &str) -> bool {
        self.palette.contains_key(key) || load::parse_hex(key).is_some()
    }

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

pub fn default_style_key(kind: &NodeKind) -> Option<&'static str> {
    crate::widget_policy::style_key(kind)
}

#[cfg(test)]
mod tests;
