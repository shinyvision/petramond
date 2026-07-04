//! Baked title/pause shell skins authored by `gui-builder`.
//!
//! Shell screens are not inventory/container GUIs, but their decorative art and
//! control hit rectangles still come from builder manifests. Runtime text and
//! dynamic rows stay code-driven.

use super::{gui_scale, HoverFit, HoverFitJson, SlotRect};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where builder-baked shell screens live. Kept separate from container GUIs so
/// their shell-only roles don't get logged as invalid inventory manifests.
const SHELL_BAKED_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/textures/gui/shell/baked"
);

static REGISTRY: OnceLock<Vec<Loaded>> = OnceLock::new();

struct Loaded {
    def: ShellDef,
    image_path: PathBuf,
    hover_path: Option<PathBuf>,
    scroll_thumb_path: Option<PathBuf>,
}

/// Which app-shell screen a baked skin is for. Matches the gui-builder's
/// `gui_type` field for shell projects.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Title,
    WorldSelect,
    /// Per-world mod enablement (+ the relocated Delete World button); opened
    /// from world-select in place of the old per-world Delete button.
    WorldSettings,
    CreateWorld,
    DeleteWorld,
    Pause,
}

impl ShellKind {
    fn default_y(self) -> f32 {
        match self {
            ShellKind::Title => 42.0,
            ShellKind::WorldSelect | ShellKind::WorldSettings => 18.0,
            ShellKind::CreateWorld => 26.0,
            ShellKind::DeleteWorld => 44.0,
            ShellKind::Pause => 54.0,
        }
    }
}

/// A non-container control rectangle authored as a builder slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellRole {
    TitleStart,
    TitleQuit,
    WorldPlay,
    WorldCreate,
    /// Geometry authored (and still baked) as the world-select Delete button;
    /// the shell now labels it "World Settings" (the delete flow moved there).
    WorldDelete,
    WorldBack,
    WorldRow,
    WorldScrollTrack,
    ModRow,
    ModScrollTrack,
    SettingsBack,
    SettingsDeleteWorld,
    CreateNameInput,
    CreateSeedInput,
    CreateCreate,
    CreateCancel,
    DeleteWorldConfirm,
    DeleteWorldCancel,
    PauseResume,
    PauseSaveQuit,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "type")]
    kind: ShellKind,
    canvas: CanvasJson,
    scale: u32,
    image: String,
    slots: Vec<SlotJson>,
    #[serde(default)]
    hover: Option<HoverJson>,
    #[serde(default)]
    tagged: Vec<TaggedJson>,
}

#[derive(Deserialize)]
struct CanvasJson {
    w: u32,
    h: u32,
}

#[derive(Deserialize)]
struct SlotJson {
    role: ShellRole,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Deserialize)]
struct HoverJson {
    image: String,
    margin: i32,
    #[serde(default)]
    fit: HoverFitJson,
    #[serde(default = "default_opacity")]
    opacity: f32,
}

#[derive(Deserialize)]
struct TaggedJson {
    tag: ShellTag,
    image: String,
    #[serde(default)]
    fit: HoverFitJson,
    #[serde(rename = "x")]
    _x: i32,
    #[serde(rename = "y")]
    _y: i32,
    #[serde(rename = "w")]
    _w: u32,
    #[serde(rename = "h")]
    _h: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ShellTag {
    WorldScrollThumb,
    #[serde(other)]
    Other,
}

fn default_opacity() -> f32 {
    1.0
}

pub struct ShellHoverDef {
    pub margin: i32,
    pub opacity: f32,
    pub fit: HoverFit,
    pub image_size: (u32, u32),
    image: String,
}

pub struct ShellImageDef {
    pub fit: HoverFit,
    pub image_size: (u32, u32),
    image: String,
}

/// One baked shell skin: logical art size plus named control rectangles.
pub struct ShellDef {
    kind: ShellKind,
    logical_w: f32,
    logical_h: f32,
    roles: HashMap<ShellRole, Vec<[f32; 4]>>,
    image: String,
    hover: Option<ShellHoverDef>,
    scroll_thumb: Option<ShellImageDef>,
}

impl ShellDef {
    fn from_manifest(s: &str) -> Option<Self> {
        let m: Manifest = serde_json::from_str(s).ok()?;
        let scale = m.scale.max(1) as f32;
        let logical = |x: i32| x as f32 / scale;
        let mut roles: HashMap<ShellRole, Vec<[f32; 4]>> = HashMap::new();
        for slot in &m.slots {
            roles.entry(slot.role).or_default().push([
                logical(slot.x),
                logical(slot.y),
                logical(slot.w),
                logical(slot.h),
            ]);
        }
        for rects in roles.values_mut() {
            rects.sort_by(|a, b| a[1].total_cmp(&b[1]).then_with(|| a[0].total_cmp(&b[0])));
        }
        Some(Self {
            kind: m.kind,
            logical_w: m.canvas.w as f32 / scale,
            logical_h: m.canvas.h as f32 / scale,
            roles,
            image: m.image,
            hover: m.hover.map(|h| ShellHoverDef {
                margin: (h.margin as f32 / scale).round() as i32,
                opacity: h.opacity,
                fit: HoverFit::from_json(h.fit, scale),
                image_size: (1, 1),
                image: h.image,
            }),
            scroll_thumb: m
                .tagged
                .into_iter()
                .find(|t| t.tag == ShellTag::WorldScrollThumb)
                .map(|t| ShellImageDef {
                    fit: HoverFit::from_json(t.fit, scale),
                    image_size: (1, 1),
                    image: t.image,
                }),
        })
    }

    fn validate(&self) -> Result<(), String> {
        let has = |role| self.roles.get(&role).is_some_and(|v| !v.is_empty());
        let required: &[ShellRole] = match self.kind {
            ShellKind::Title => &[ShellRole::TitleStart, ShellRole::TitleQuit],
            ShellKind::WorldSelect => &[
                ShellRole::WorldPlay,
                ShellRole::WorldCreate,
                ShellRole::WorldDelete,
                ShellRole::WorldBack,
                ShellRole::WorldRow,
                ShellRole::WorldScrollTrack,
            ],
            ShellKind::WorldSettings => &[
                ShellRole::ModRow,
                ShellRole::ModScrollTrack,
                ShellRole::SettingsBack,
                ShellRole::SettingsDeleteWorld,
            ],
            ShellKind::CreateWorld => &[
                ShellRole::CreateNameInput,
                ShellRole::CreateSeedInput,
                ShellRole::CreateCreate,
                ShellRole::CreateCancel,
            ],
            ShellKind::DeleteWorld => {
                &[ShellRole::DeleteWorldConfirm, ShellRole::DeleteWorldCancel]
            }
            ShellKind::Pause => &[ShellRole::PauseResume, ShellRole::PauseSaveQuit],
        };
        for &role in required {
            if !has(role) {
                return Err(format!("{:?} missing {role:?}", self.kind));
            }
        }
        if matches!(self.kind, ShellKind::WorldSelect | ShellKind::WorldSettings)
            && self.scroll_thumb.is_none()
        {
            return Err(format!("{:?} missing world scroll thumb", self.kind));
        }
        Ok(())
    }

    fn placement(&self, screen: (u32, u32)) -> (f32, f32, f32) {
        let s = gui_scale(screen);
        let w = self.logical_w * s;
        let x = (screen.0 as f32 - w) * 0.5;
        let y = self.kind.default_y() * s;
        (x, y, s)
    }

    pub fn panel_rect(&self, screen: (u32, u32)) -> SlotRect {
        let (x, y, s) = self.placement(screen);
        SlotRect {
            x,
            y,
            w: self.logical_w * s,
            h: self.logical_h * s,
        }
    }

    pub fn role_rect(&self, role: ShellRole, screen: (u32, u32)) -> Option<SlotRect> {
        self.role_rects(role, screen).into_iter().next()
    }

    pub fn role_rects(&self, role: ShellRole, screen: (u32, u32)) -> Vec<SlotRect> {
        let (ox, oy, s) = self.placement(screen);
        self.roles
            .get(&role)
            .into_iter()
            .flat_map(|rects| rects.iter())
            .map(|r| SlotRect {
                x: ox + r[0] * s,
                y: oy + r[1] * s,
                w: r[2] * s,
                h: r[3] * s,
            })
            .collect()
    }

    pub fn hover(&self) -> Option<&ShellHoverDef> {
        self.hover.as_ref()
    }

    pub fn scroll_thumb(&self) -> Option<&ShellImageDef> {
        self.scroll_thumb.as_ref()
    }
}

fn load_baked() -> Vec<Loaded> {
    let dir = Path::new(SHELL_BAKED_DIR);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(mut def) = ShellDef::from_manifest(&text) else {
            eprintln!(
                "shell gui: ignoring unparseable manifest {}",
                path.display()
            );
            continue;
        };
        if let Err(e) = def.validate() {
            eprintln!("shell gui: ignoring {} - {e}", path.display());
            continue;
        }
        let image_path = dir.join(&def.image);
        if !image_path.exists() {
            eprintln!(
                "shell gui: ignoring {} - missing panel art {}",
                path.display(),
                def.image
            );
            continue;
        }
        let hover_path = def
            .hover
            .as_ref()
            .map(|h| dir.join(&h.image))
            .filter(|p| p.exists());
        if let (Some(hover), Some(path)) = (def.hover.as_mut(), hover_path.as_ref()) {
            if let Ok((w, h)) = image::image_dimensions(path) {
                hover.image_size = (w, h);
            }
        }
        let scroll_thumb_path = def
            .scroll_thumb
            .as_ref()
            .map(|h| dir.join(&h.image))
            .filter(|p| p.exists());
        if let (Some(thumb), Some(path)) = (def.scroll_thumb.as_mut(), scroll_thumb_path.as_ref()) {
            if let Ok((w, h)) = image::image_dimensions(path) {
                thumb.image_size = (w, h);
            }
        }
        if matches!(def.kind, ShellKind::WorldSelect | ShellKind::WorldSettings)
            && scroll_thumb_path.is_none()
        {
            eprintln!(
                "shell gui: ignoring {} - missing world scroll thumb art",
                path.display()
            );
            continue;
        }
        out.push(Loaded {
            def,
            image_path,
            hover_path,
            scroll_thumb_path,
        });
    }
    out
}

fn registry() -> &'static [Loaded] {
    REGISTRY.get_or_init(load_baked)
}

pub(crate) fn shell_def(kind: ShellKind) -> Option<&'static ShellDef> {
    registry()
        .iter()
        .find(|l| l.def.kind == kind)
        .map(|l| &l.def)
}

pub(crate) fn baked_shell_skins() -> Vec<(ShellKind, PathBuf)> {
    registry()
        .iter()
        .map(|l| (l.def.kind, l.image_path.clone()))
        .collect()
}

pub(crate) fn baked_shell_hovers() -> Vec<(ShellKind, PathBuf)> {
    registry()
        .iter()
        .filter_map(|l| l.hover_path.clone().map(|p| (l.def.kind, p)))
        .collect()
}

pub(crate) fn baked_shell_scroll_thumbs() -> Vec<(ShellKind, PathBuf)> {
    registry()
        .iter()
        .filter_map(|l| l.scroll_thumb_path.clone().map(|p| (l.def.kind, p)))
        .collect()
}
