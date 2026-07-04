//! The data-driven GUI model: every screen (hotbar HUD, inventory, crafting
//! table, furnace, chest — and mod-defined GUIs) is a baked PNG panel + a JSON
//! manifest of typed slots and widgets. Engine manifests are authored in the
//! `gui-builder` tool; mod GUI manifests are hand-authored JSON (the builder
//! does not know the Phase 5 widget schema yet — see WIKI/gui.md). This module
//! owns the parsed model and ALL of its layout math; both the renderer UI
//! builder and the App's click hit-test read the SAME [`GuiDef`] so what's
//! drawn and what's clicked can never diverge.
//!
//! Baked GUIs are loaded from a directory at RUNTIME (not embedded) so re-baking
//! from the gui-builder + restarting picks them up with no recompile, and so the
//! optional hover / tagged-overlay PNGs can be present-or-not without a build
//! error. The dir is resolved at compile time relative to the crate root, so it
//! works regardless of the working directory.
//!
//! The model is generic over [`GuiKind`] and slot [`Role`]: a [`GuiDef`] holds
//! the logical panel size, per-role slot rects, an optional hover highlight,
//! optional dynamic string-tagged overlays (the furnace's smelt arrow / burn
//! flame; mod gauges), and — for mod kinds — the Phase 5 widgets
//! (`label`/`image`/`button`/`rotimage`). Manifest coordinates are baked-canvas
//! pixels (authoring scale); they're converted once to *logical* pixels so the
//! game applies its own integer `gui_scale` on top — every screen scales
//! identically.
//!
//! ## The role→index contract
//! A manifest lists a role's slots in a stable order; the i-th slot of a role maps
//! to the i-th game slot of that role's domain (storage→chest slot i, player_inv→
//! inventory slot 9+i, hotbar→inventory slot i, craft_input→craft cell i). That
//! order MUST be row-major. [`GuiDef::validate`] enforces both the per-kind slot
//! counts and the row-major ordering at load, so a future re-bake can never
//! silently mis-route a click.
//!
//! ## Mod GUI kinds
//! A manifest may declare `"type": "mod_id:name"`; the kind registers in the
//! runtime kind registry ([`super::kind`]) and MUST carry the namespace of the
//! pack that ships the manifest (foreign-namespace manifests are skipped, like
//! foreign catalog keys disable a pack). Mod kinds carry NO role slots in
//! Phase 5 — widgets only; [`GuiDef::validate`] validates the widgets instead
//! of slot counts.

use super::{gui_scale, GuiKind, HoverFit, HoverFitJson, MenuSlot, Role, SlotRect, WidgetId};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Where baked GUIs live, relative to the asset roots (the gui-builder bakes
/// into the dev tree's copy). Resolved through [`crate::assets`], so a shipped
/// install finds them beside the executable and a mod pack can shadow one by
/// baking a manifest of the same file name.
const BAKED_DIR: &str = "textures/gui/baked";

/// The closed-HUD hotbar's gap from the screen bottom (logical px), scaled by
/// `gui_scale`. Matches the classic 1px lift so the held-item hand clears it.
const HOTBAR_BOTTOM_MARGIN: f32 = 1.0;

static REGISTRY: OnceLock<Vec<Loaded>> = OnceLock::new();

/// A GUI texture's key within its kind: the interned image FILE NAME an
/// overlay or widget names. The renderer binds one texture per (kind, key).
pub(crate) type SpriteKey = &'static str;

/// One baked GUI: its parsed def plus the resolved on-disk paths the renderer
/// loads its panel / hover / sprite textures from.
struct Loaded {
    def: GuiDef,
    panel_path: PathBuf,
    hover_path: Option<PathBuf>,
    /// Every overlay/widget image beside the manifest, keyed by file name.
    sprite_paths: Vec<(SpriteKey, PathBuf)>,
}

fn load_baked() -> Vec<Loaded> {
    // Overlay the baked dirs (base + packs) by manifest FILE NAME: the
    // highest-priority copy of a name wins, new names add. Each manifest's art
    // resolves beside the manifest itself, so a pack GUI is self-contained.
    // The owning pack id rides along for the mod-kind namespace check.
    struct Found {
        json: PathBuf,
        dir: PathBuf,
        pack_id: Option<String>,
    }
    let mut manifests: Vec<(String, Found)> = Vec::new();
    for (dir, pack_id) in crate::assets::layer_dirs_with_ids(BAKED_DIR) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let found = Found {
                json: path,
                dir: dir.clone(),
                pack_id: pack_id.clone(),
            };
            match manifests.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = found,
                None => manifests.push((name, found)),
            }
        }
    }
    // Deterministic registry order regardless of directory iteration order.
    manifests.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = Vec::new();
    for (_, found) in manifests {
        let (path, dir) = (found.json, found.dir.as_path());
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(def) = GuiDef::from_manifest(&text) else {
            eprintln!("gui: ignoring unparseable manifest {}", path.display());
            continue;
        };
        if let Err(e) = kind_permitted(def.kind, found.pack_id.as_deref()) {
            eprintln!("gui: ignoring {} — {e}", path.display());
            continue;
        }
        if let Err(e) = def.validate() {
            eprintln!("gui: ignoring {} — {e}", path.display());
            continue;
        }
        let panel_path = dir.join(&def.image);
        if !panel_path.exists() {
            eprintln!(
                "gui: ignoring {} — missing panel art {}",
                path.display(),
                def.image
            );
            continue;
        }
        let mut def = def;
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
        let mut sprite_paths: Vec<(SpriteKey, PathBuf)> = Vec::new();
        for key in def.sprite_keys() {
            if sprite_paths.iter().any(|(k, _)| *k == key) {
                continue;
            }
            let p = dir.join(key);
            if p.exists() {
                sprite_paths.push((key, p));
            } else {
                eprintln!(
                    "gui: {} names missing art {key}; the quad will not draw",
                    path.display()
                );
            }
        }
        out.push(Loaded {
            def,
            panel_path,
            hover_path,
            sprite_paths,
        });
    }
    out
}

/// The mod-kind ownership rule: a namespaced manifest kind must carry the id
/// of the pack that ships the manifest (base dirs ship no namespace). Engine
/// kinds are always permitted (a pack may re-skin the furnace).
fn kind_permitted(kind: GuiKind, pack_id: Option<&str>) -> Result<(), String> {
    if !kind.is_mod() {
        return Ok(());
    }
    let key = super::kind_key(kind).unwrap_or("?");
    let owner = key.split_once(':').map(|(ns, _)| ns).unwrap_or("");
    match pack_id {
        Some(id) if id == owner => Ok(()),
        Some(id) => Err(format!(
            "kind '{key}' does not belong to pack '{id}' (namespaced kinds must use the \
             shipping pack's own id)"
        )),
        None => Err(format!(
            "kind '{key}' is namespaced but the manifest ships outside any pack"
        )),
    }
}

fn registry() -> &'static [Loaded] {
    REGISTRY.get_or_init(load_baked)
}

/// The def for `kind`, or `None` if no (valid) manifest is baked for it.
pub(crate) fn def(kind: GuiKind) -> Option<&'static GuiDef> {
    registry()
        .iter()
        .find(|l| l.def.kind == kind)
        .map(|l| &l.def)
}

/// (kind, panel PNG path) for every baked GUI — the renderer uploads each into a
/// texture + bind group keyed by [`GuiTexId::Panel`].
pub(crate) fn baked_panels() -> Vec<(GuiKind, PathBuf)> {
    registry()
        .iter()
        .map(|l| (l.def.kind, l.panel_path.clone()))
        .collect()
}

/// (kind, hover PNG path) for every baked GUI that declares a hover highlight.
pub(crate) fn baked_hovers() -> Vec<(GuiKind, PathBuf)> {
    registry()
        .iter()
        .filter_map(|l| l.hover_path.clone().map(|p| (l.def.kind, p)))
        .collect()
}

/// (kind, sprite key, PNG path) for every overlay/widget image a baked GUI
/// names (furnace gauges, mod widget art).
pub(crate) fn baked_sprites() -> Vec<(GuiKind, SpriteKey, PathBuf)> {
    registry()
        .iter()
        .flat_map(|l| {
            l.sprite_paths
                .iter()
                .map(move |(k, p)| (l.def.kind, *k, p.clone()))
        })
        .collect()
}

/// The logical slot under the cursor in `kind`'s layout. `None` if the cursor is
/// over no slot (or `kind` has no baked manifest). Buttons hit-test like slots:
/// a `button` widget resolves to [`MenuSlot::Widget`].
pub(crate) fn hit(kind: GuiKind, screen: (u32, u32), cursor: (f32, f32)) -> Option<MenuSlot> {
    let def = def(kind)?;
    if let Some(id) = def.button_at(screen, cursor) {
        return Some(MenuSlot::Widget(id));
    }
    let (role, i) = def.role_at_any(screen, cursor)?;
    role.menu_slot(i)
}

/// Whether the cursor lies over `kind`'s panel rect (used to decide whether an
/// off-slot click throws the held stack vs does nothing on the panel art).
pub(crate) fn panel_contains(kind: GuiKind, screen: (u32, u32), cursor: (f32, f32)) -> bool {
    def(kind).is_some_and(|d| d.panel_rect(screen).contains(cursor.0, cursor.1))
}

// ---- manifest JSON (the gui-builder bake format + Phase 5 widgets) ---------

/// How a dynamic overlay clips against its `0..=1` fraction. The two modes are
/// the (previously furnace-hardcoded) fill directions, now declared per
/// overlay row; the legacy furnace tags default their historical mode so
/// existing bakes render identically without a re-bake.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OverlayMode {
    /// Grows left→right with the fraction (the smelt arrow).
    GrowLr,
    /// Depletes top→down: the bottom `frac` stays visible (the burn flame).
    DepleteTd,
}

#[derive(Deserialize)]
struct Manifest {
    /// The kind key: an engine name (`"furnace"`) or a namespaced mod kind
    /// (`"wheel:wheel"`). Resolved (and, for namespaced keys, registered)
    /// against the runtime kind registry.
    #[serde(rename = "type")]
    kind: String,
    canvas: CanvasJson,
    scale: u32,
    image: String,
    #[serde(default)]
    slots: Vec<SlotJson>,
    #[serde(default)]
    hover: Option<HoverJson>,
    #[serde(default)]
    tagged: Vec<TaggedJson>,
    #[serde(default)]
    widgets: Vec<WidgetJson>,
}

#[derive(Deserialize)]
struct CanvasJson {
    w: u32,
    h: u32,
}

#[derive(Deserialize)]
struct SlotJson {
    role: Role,
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
    tag: String,
    image: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// Absent in legacy bakes: the furnace tags default their historical
    /// direction (`furnace_arrow` grows, `furnace_flame` depletes); anything
    /// else defaults to `grow_lr`.
    #[serde(default)]
    mode: Option<OverlayMode>,
}

/// Phase 5 widget rows (mod GUIs). Coordinates are canvas px like slots.
#[derive(Deserialize)]
#[serde(tag = "widget", rename_all = "snake_case")]
enum WidgetJson {
    /// Text via the runtime text pipeline: `state_key` reads the GUI state map
    /// (dynamic), `text` is the static fallback when the key is absent.
    Label {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        state_key: Option<String>,
        #[serde(default)]
        align: Option<LabelAlign>,
        #[serde(default)]
        color: Option<[f32; 4]>,
    },
    /// A static PNG beside the manifest.
    Image {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        image: String,
    },
    /// A hit-testable rect; a click latches [`MenuSlot::Widget`] and the tick
    /// dispatches it to the kind's owning mod. `image` is optional decoration
    /// (the button may be baked into the panel art); `hover_image` replaces
    /// the shared hover highlight while the cursor is over the button.
    Button {
        id: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        #[serde(default)]
        image: Option<String>,
        #[serde(default)]
        hover_image: Option<String>,
    },
    /// A textured quad rotated at draw time by the angle (radians) read from
    /// the GUI state map at `state_key`. `pivot` is canvas px relative to the
    /// widget rect's top-left; absent = the rect centre.
    Rotimage {
        #[serde(default)]
        #[allow(dead_code)] // reserved: rotimages are not hit-testable in Phase 5
        id: Option<String>,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        image: String,
        #[serde(default)]
        pivot: Option<[f32; 2]>,
        state_key: String,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LabelAlign {
    Left,
    Center,
}

fn default_opacity() -> f32 {
    1.0
}

/// The hover highlight for a GUI: the graphic + how far it extends beyond a slot.
pub(crate) struct HoverDef {
    /// Logical px the highlight extends beyond the slot on every side.
    pub margin: i32,
    pub opacity: f32,
    pub fit: HoverFit,
    pub image_size: (u32, u32),
    /// Panel-relative PNG filename (resolved to a path in [`load_baked`]).
    image: String,
}

/// A dynamic overlay's placement: its tag (the fraction source — FurnaceView
/// for the furnace tags, the GUI state map otherwise), clip mode, art, and
/// logical rect.
pub(crate) struct OverlayDef {
    pub tag: &'static str,
    pub mode: OverlayMode,
    /// The interned image file name — the renderer's sprite bind key.
    pub image: SpriteKey,
    /// `[x, y, w, h]` in logical px (canvas px ÷ authoring scale).
    base: [f32; 4],
}

/// A parsed Phase 5 widget in logical px.
pub(crate) enum WidgetDef {
    Label {
        base: [f32; 4],
        text: Option<String>,
        state_key: Option<String>,
        align: LabelAlign,
        color: [f32; 4],
    },
    Image {
        base: [f32; 4],
        image: SpriteKey,
    },
    Button {
        base: [f32; 4],
        id: WidgetId,
        image: Option<SpriteKey>,
        hover_image: Option<SpriteKey>,
    },
    Rotimage {
        base: [f32; 4],
        image: SpriteKey,
        /// Logical px from the rect's top-left; `None` = rect centre.
        pivot: Option<[f32; 2]>,
        state_key: String,
    },
}

/// How a panel is anchored on screen. Menus centre; the hotbar HUD pins to the
/// bottom edge.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Anchor {
    Center,
    BottomCenter,
}

/// A parsed data-driven GUI: logical panel size (canvas ÷ authoring scale) plus
/// per-role slot rects in *logical* (base) pixels, ready to centre + scale by the
/// game's integer `gui_scale`.
pub(crate) struct GuiDef {
    kind: GuiKind,
    logical_w: f32,
    logical_h: f32,
    roles: HashMap<Role, Vec<[f32; 4]>>,
    overlays: Vec<OverlayDef>,
    widgets: Vec<WidgetDef>,
    /// Panel PNG filename (resolved to a path in [`load_baked`]).
    image: String,
    hover: Option<HoverDef>,
}

impl GuiDef {
    fn from_manifest(s: &str) -> Option<GuiDef> {
        let m: Manifest = serde_json::from_str(s).ok()?;
        // Engine name or namespaced mod kind (registered on first sight); an
        // unknown bare name resolves to Other and is rejected by validate().
        let kind = super::intern_kind(&m.kind).unwrap_or(GuiKind::Other);
        let scale = m.scale.max(1) as f32;
        let logical = |x: i32| x as f32 / scale;
        // Manifest coords are baked-canvas pixels (authoring scale); convert to
        // logical/base pixels so the game applies its own gui_scale on top.
        let rect =
            |x: i32, y: i32, w: i32, h: i32| [logical(x), logical(y), logical(w), logical(h)];
        let mut roles: HashMap<Role, Vec<[f32; 4]>> = HashMap::new();
        for s in &m.slots {
            roles
                .entry(s.role)
                .or_default()
                .push(rect(s.x, s.y, s.w, s.h));
        }
        let overlays = m
            .tagged
            .into_iter()
            .map(|t| OverlayDef {
                mode: t.mode.unwrap_or(match t.tag.as_str() {
                    "furnace_flame" => OverlayMode::DepleteTd,
                    _ => OverlayMode::GrowLr,
                }),
                tag: super::intern_str(&t.tag),
                image: super::intern_str(&t.image),
                base: rect(t.x, t.y, t.w, t.h),
            })
            .collect();
        let widgets = m
            .widgets
            .into_iter()
            .map(|w| match w {
                WidgetJson::Label {
                    x,
                    y,
                    w,
                    h,
                    text,
                    state_key,
                    align,
                    color,
                } => WidgetDef::Label {
                    base: rect(x, y, w, h),
                    text,
                    state_key,
                    align: align.unwrap_or(LabelAlign::Left),
                    color: color.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                },
                WidgetJson::Image { x, y, w, h, image } => WidgetDef::Image {
                    base: rect(x, y, w, h),
                    image: super::intern_str(&image),
                },
                WidgetJson::Button {
                    id,
                    x,
                    y,
                    w,
                    h,
                    image,
                    hover_image,
                } => WidgetDef::Button {
                    base: rect(x, y, w, h),
                    id: super::intern_str(&id),
                    image: image.as_deref().map(super::intern_str),
                    hover_image: hover_image.as_deref().map(super::intern_str),
                },
                WidgetJson::Rotimage {
                    id: _,
                    x,
                    y,
                    w,
                    h,
                    image,
                    pivot,
                    state_key,
                } => WidgetDef::Rotimage {
                    base: rect(x, y, w, h),
                    image: super::intern_str(&image),
                    pivot: pivot.map(|p| [p[0] / scale, p[1] / scale]),
                    state_key,
                },
            })
            .collect();
        let hover = m.hover.map(|h| HoverDef {
            margin: (h.margin as f32 / scale).round() as i32,
            opacity: h.opacity,
            fit: HoverFit::from_json(h.fit, scale),
            image_size: (1, 1),
            image: h.image,
        });
        Some(GuiDef {
            kind,
            logical_w: m.canvas.w as f32 / scale,
            logical_h: m.canvas.h as f32 / scale,
            roles,
            overlays,
            widgets,
            image: m.image,
            hover,
        })
    }

    /// Enforce the load contracts. Engine kinds: the role→index contract (the
    /// expected per-role slot counts and row-major ordering — a mismatch means
    /// a bad bake; the caller skips the manifest rather than silently
    /// mis-routing). Mod kinds carry NO role slots in Phase 5; their widgets
    /// are validated instead.
    fn validate(&self) -> Result<(), String> {
        if self.kind.is_mod() {
            return self.validate_mod_widgets();
        }
        let n = |r: Role| self.role_slots(r).len();
        let want: &[(Role, usize)] = match self.kind {
            GuiKind::Chest => &[
                (Role::Storage, 27),
                (Role::PlayerInv, 27),
                (Role::Hotbar, 9),
            ],
            GuiKind::Inventory => &[
                (Role::PlayerInv, 27),
                (Role::Hotbar, 9),
                (Role::CraftInput, 4),
                (Role::CraftResult, 1),
            ],
            GuiKind::CraftingTable => &[
                (Role::PlayerInv, 27),
                (Role::Hotbar, 9),
                (Role::CraftInput, 9),
                (Role::CraftResult, 1),
            ],
            GuiKind::Furnace => &[
                (Role::PlayerInv, 27),
                (Role::Hotbar, 9),
                (Role::FurnaceInput, 1),
                (Role::FurnaceFuel, 1),
                (Role::FurnaceOutput, 1),
            ],
            GuiKind::Hotbar => &[(Role::Hotbar, 9)],
            GuiKind::FurnitureWorkbench => &[
                (Role::PlayerInv, 27),
                (Role::Hotbar, 9),
                (Role::WorkbenchInput, 1),
                (Role::WorkbenchResult, 21),
            ],
            _ => return Err("unknown gui type".to_string()),
        };
        for &(role, count) in want {
            if n(role) != count {
                return Err(format!(
                    "{:?} wants {count} {role:?} slots, found {}",
                    self.kind,
                    n(role)
                ));
            }
        }
        // Multi-slot roles must be row-major so in-role index == game slot index.
        for role in [
            Role::Storage,
            Role::PlayerInv,
            Role::Hotbar,
            Role::CraftInput,
            Role::WorkbenchResult,
        ] {
            check_row_major(role, self.role_slots(role))?;
        }
        Ok(())
    }

    /// The Phase 5 mod-kind contract: no role slots (widgets only), unique
    /// non-empty button ids, and every state-bound widget names its source.
    fn validate_mod_widgets(&self) -> Result<(), String> {
        if !self.roles.is_empty() {
            return Err("mod GUI kinds carry no role slots (widgets only in Phase 5)".into());
        }
        let mut button_ids: Vec<WidgetId> = Vec::new();
        for w in &self.widgets {
            match w {
                WidgetDef::Button { id, .. } => {
                    if id.is_empty() {
                        return Err("button widget with an empty id".into());
                    }
                    if button_ids.contains(id) {
                        return Err(format!("duplicate button id '{id}'"));
                    }
                    button_ids.push(id);
                }
                WidgetDef::Label {
                    text, state_key, ..
                } => {
                    if text.is_none() && state_key.is_none() {
                        return Err("label widget needs 'text' or 'state_key'".into());
                    }
                }
                WidgetDef::Rotimage { state_key, .. } => {
                    if state_key.is_empty() {
                        return Err("rotimage widget with an empty state_key".into());
                    }
                }
                WidgetDef::Image { .. } => {}
            }
        }
        Ok(())
    }

    fn anchor(&self) -> Anchor {
        match self.kind {
            GuiKind::Hotbar => Anchor::BottomCenter,
            _ => Anchor::Center,
        }
    }

    /// `(offset_x, offset_y, scale)` placing the panel on `screen`.
    fn placement(&self, screen: (u32, u32)) -> (f32, f32, f32) {
        let s = gui_scale(screen);
        let (w, h) = (screen.0 as f32, screen.1 as f32);
        let (lw, lh) = (self.logical_w * s, self.logical_h * s);
        let (ox, oy) = match self.anchor() {
            Anchor::Center => ((w - lw) * 0.5, (h - lh) * 0.5),
            Anchor::BottomCenter => ((w - lw) * 0.5, h - lh - HOTBAR_BOTTOM_MARGIN * s),
        };
        (ox, oy, s)
    }

    fn rect(&self, base: [f32; 4], screen: (u32, u32)) -> SlotRect {
        let (ox, oy, s) = self.placement(screen);
        SlotRect {
            x: ox + base[0] * s,
            y: oy + base[1] * s,
            w: base[2] * s,
            h: base[3] * s,
        }
    }

    fn role_slots(&self, role: Role) -> &[[f32; 4]] {
        self.roles.get(&role).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub(crate) fn panel_rect(&self, screen: (u32, u32)) -> SlotRect {
        self.rect([0.0, 0.0, self.logical_w, self.logical_h], screen)
    }

    pub(crate) fn hover(&self) -> Option<&HoverDef> {
        self.hover.as_ref()
    }

    /// Screen rect of slot `i` of `role`, or `None` if out of range.
    pub(crate) fn role_rect(&self, role: Role, i: usize, screen: (u32, u32)) -> Option<SlotRect> {
        self.role_slots(role).get(i).map(|b| self.rect(*b, screen))
    }

    /// Visit every dynamic overlay with its screen rect, in manifest order.
    pub(crate) fn for_each_overlay(
        &self,
        screen: (u32, u32),
        mut f: impl FnMut(&OverlayDef, SlotRect),
    ) {
        for o in &self.overlays {
            f(o, self.rect(o.base, screen));
        }
    }

    /// Screen rect of the dynamic overlay `tag`, or `None` if this GUI has none.
    #[cfg(test)]
    pub(crate) fn overlay_rect(&self, tag: &str, screen: (u32, u32)) -> Option<SlotRect> {
        self.overlays
            .iter()
            .find(|o| o.tag == tag)
            .map(|o| self.rect(o.base, screen))
    }

    /// Visit every Phase 5 widget with its screen rect, in manifest order.
    pub(crate) fn for_each_widget(
        &self,
        screen: (u32, u32),
        mut f: impl FnMut(&WidgetDef, SlotRect),
    ) {
        for w in &self.widgets {
            let base = match w {
                WidgetDef::Label { base, .. }
                | WidgetDef::Image { base, .. }
                | WidgetDef::Button { base, .. }
                | WidgetDef::Rotimage { base, .. } => *base,
            };
            f(w, self.rect(base, screen));
        }
    }

    /// The button widget under the cursor, if any.
    pub(crate) fn button_at(&self, screen: (u32, u32), cursor: (f32, f32)) -> Option<WidgetId> {
        for w in &self.widgets {
            if let WidgetDef::Button { base, id, .. } = w {
                if self.rect(*base, screen).contains(cursor.0, cursor.1) {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Every image file name this GUI's overlays/widgets reference.
    fn sprite_keys(&self) -> Vec<SpriteKey> {
        let mut keys: Vec<SpriteKey> = self.overlays.iter().map(|o| o.image).collect();
        for w in &self.widgets {
            match w {
                WidgetDef::Image { image, .. } | WidgetDef::Rotimage { image, .. } => {
                    keys.push(image)
                }
                WidgetDef::Button {
                    image, hover_image, ..
                } => {
                    keys.extend(image.iter().copied());
                    keys.extend(hover_image.iter().copied());
                }
                WidgetDef::Label { .. } => {}
            }
        }
        keys
    }

    /// Visit every slot of every role with its screen rect (for emitting icons).
    pub(crate) fn for_each_slot(
        &self,
        screen: (u32, u32),
        mut f: impl FnMut(Role, usize, SlotRect),
    ) {
        for (&role, rects) in &self.roles {
            for (i, b) in rects.iter().enumerate() {
                f(role, i, self.rect(*b, screen));
            }
        }
    }

    /// The (role, in-role index) of the slot under the cursor, or `None`. Slots
    /// never overlap, so the first containing rect is unambiguous.
    pub(crate) fn role_at_any(
        &self,
        screen: (u32, u32),
        cursor: (f32, f32),
    ) -> Option<(Role, usize)> {
        for (&role, rects) in &self.roles {
            for (i, b) in rects.iter().enumerate() {
                if self.rect(*b, screen).contains(cursor.0, cursor.1) {
                    return Some((role, i));
                }
            }
        }
        None
    }

    /// The screen rect of the slot under the cursor — used to place the hover
    /// highlight (inflated by the hover margin).
    pub(crate) fn hovered_slot_rect(
        &self,
        screen: (u32, u32),
        cursor: (f32, f32),
    ) -> Option<SlotRect> {
        self.role_at_any(screen, cursor)
            .and_then(|(role, i)| self.role_rect(role, i, screen))
    }
}

/// Verify a role's slot rects run top-to-bottom, left-to-right (row-major), so the
/// in-role index lines up with the game's row-major slot index.
fn check_row_major(role: Role, rects: &[[f32; 4]]) -> Result<(), String> {
    for w in rects.windows(2) {
        let (a, b) = (w[0], w[1]);
        // Same row when the y's are within half a slot height; then x must advance.
        let same_row = (a[1] - b[1]).abs() < a[3] * 0.5;
        let ok = if same_row { b[0] > a[0] } else { b[1] > a[1] };
        if !ok {
            return Err(format!("{role:?} slots are not in row-major order"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::{CraftHit, FurnaceHit};
    use crate::inventory::HOTBAR_LEN;

    // Inline samples so tests exercise the parser/geometry without pinning the
    // user's freely-re-baked manifests (their exact slot pixels are table data).
    const CHEST: &str = r#"{
        "type": "chest",
        "canvas": { "w": 352, "h": 332 },
        "scale": 2,
        "image": "chest.png",
        "slots": [
            { "role": "storage", "x": 16, "y": 26, "w": 32, "h": 32 },
            { "role": "storage", "x": 52, "y": 26, "w": 32, "h": 32 },
            { "role": "hotbar", "x": 16, "y": 300, "w": 32, "h": 32 },
            { "role": "player_inv", "x": 16, "y": 200, "w": 32, "h": 32 }
        ],
        "hover": { "image": "chest_hover.png", "margin": 8, "fit": { "mode": "stretch" }, "opacity": 0.5 }
    }"#;

    fn furnace_with_overlays() -> &'static str {
        r#"{
            "type": "furnace",
            "canvas": { "w": 352, "h": 332 },
            "scale": 2,
            "image": "furnace.png",
            "slots": [
                { "role": "furnace_input", "x": 162, "y": 25, "w": 28, "h": 28 }
            ],
            "tagged": [
                { "tag": "furnace_arrow", "image": "a.png", "x": 205, "y": 62, "w": 24, "h": 16 },
                { "tag": "furnace_flame", "image": "f.png", "x": 166, "y": 63, "w": 20, "h": 20 }
            ]
        }"#
    }

    fn mod_gui() -> &'static str {
        r#"{
            "type": "layouttest:wheel",
            "canvas": { "w": 96, "h": 64 },
            "scale": 1,
            "image": "panel.png",
            "widgets": [
                { "widget": "label", "x": 8, "y": 4, "w": 80, "h": 10, "text": "Spin!", "state_key": "layouttest:result", "align": "center" },
                { "widget": "image", "x": 4, "y": 4, "w": 8, "h": 8, "image": "deco.png" },
                { "widget": "button", "id": "spin", "x": 32, "y": 44, "w": 32, "h": 14, "image": "btn.png", "hover_image": "btn_h.png" },
                { "widget": "rotimage", "x": 32, "y": 16, "w": 32, "h": 32, "image": "wheel.png", "pivot": [16, 16], "state_key": "layouttest:angle" }
            ]
        }"#
    }

    #[test]
    fn manifest_parses_roles_and_logical_size() {
        let def = GuiDef::from_manifest(CHEST).unwrap();
        assert_eq!(def.kind, GuiKind::Chest);
        assert_eq!(def.role_slots(Role::Storage).len(), 2);
        assert_eq!(def.role_slots(Role::Hotbar).len(), 1);
        // 352x332 baked at scale 2 -> 176x166 logical.
        assert_eq!((def.logical_w, def.logical_h), (176.0, 166.0));
    }

    #[test]
    fn hover_block_converts_margin_to_logical_px() {
        let h = GuiDef::from_manifest(CHEST).unwrap().hover.unwrap();
        assert_eq!(h.margin, 4); // 8 canvas px / scale 2 = 4 logical px
        assert_eq!(h.opacity, 0.5);
        assert_eq!(h.image, "chest_hover.png");
    }

    #[test]
    fn tagged_overlays_parse_to_logical_rects_with_legacy_modes() {
        let def = GuiDef::from_manifest(furnace_with_overlays()).unwrap();
        let screen = (1280, 720);
        // furnace_arrow (205,62,24,16) at scale 2 -> logical (102.5,31,12,8).
        let r = def.overlay_rect("furnace_arrow", screen).unwrap();
        let (ox, oy, s) = def.placement(screen);
        assert!((r.x - (ox + 102.5 * s)).abs() < 0.01);
        assert!((r.y - (oy + 31.0 * s)).abs() < 0.01);
        assert!((r.w - 12.0 * s).abs() < 0.01);
        assert!(def.overlay_rect("furnace_flame", screen).is_some());
        // A legacy bake (no "mode" field) keeps the furnace's historical fill
        // directions — this is what keeps existing furnace visuals identical.
        let modes: Vec<(&str, OverlayMode)> =
            def.overlays.iter().map(|o| (o.tag, o.mode)).collect();
        assert_eq!(
            modes,
            vec![
                ("furnace_arrow", OverlayMode::GrowLr),
                ("furnace_flame", OverlayMode::DepleteTd),
            ]
        );
    }

    #[test]
    fn mod_kind_manifest_parses_widgets_and_validates() {
        let def = GuiDef::from_manifest(mod_gui()).unwrap();
        assert!(def.kind.is_mod());
        assert_eq!(crate::gui::kind_key(def.kind), Some("layouttest:wheel"));
        def.validate().expect("widgets-only mod manifest validates");
        assert_eq!(def.widgets.len(), 4);
        // Button hit-test resolves to the widget id; off-button misses.
        let screen = (1280, 720);
        let (ox, oy, s) = def.placement(screen);
        let center = (ox + (32.0 + 16.0) * s, oy + (44.0 + 7.0) * s);
        assert_eq!(def.button_at(screen, center), Some("spin"));
        assert_eq!(def.button_at(screen, (ox - 5.0, oy - 5.0)), None);
        // Sprite keys gather every referenced image once.
        let mut keys = def.sprite_keys();
        keys.sort_unstable();
        assert_eq!(keys, vec!["btn.png", "btn_h.png", "deco.png", "wheel.png"]);
    }

    #[test]
    fn mod_kind_rejects_role_slots_and_bad_widgets() {
        // Role slots on a mod kind are refused (widgets only in Phase 5).
        let with_slots = r#"{
            "type": "layouttest:slotted", "canvas": { "w": 32, "h": 32 }, "scale": 1, "image": "p.png",
            "slots": [ { "role": "hotbar", "x": 0, "y": 0, "w": 16, "h": 16 } ]
        }"#;
        let def = GuiDef::from_manifest(with_slots).unwrap();
        assert!(def.validate().unwrap_err().contains("no role slots"));

        // Duplicate button ids are refused.
        let dup = r#"{
            "type": "layouttest:dup", "canvas": { "w": 32, "h": 32 }, "scale": 1, "image": "p.png",
            "widgets": [
                { "widget": "button", "id": "a", "x": 0, "y": 0, "w": 8, "h": 8 },
                { "widget": "button", "id": "a", "x": 8, "y": 0, "w": 8, "h": 8 }
            ]
        }"#;
        let def = GuiDef::from_manifest(dup).unwrap();
        assert!(def.validate().unwrap_err().contains("duplicate button id"));

        // A label bound to nothing is refused.
        let blank = r#"{
            "type": "layouttest:blank", "canvas": { "w": 32, "h": 32 }, "scale": 1, "image": "p.png",
            "widgets": [ { "widget": "label", "x": 0, "y": 0, "w": 8, "h": 8 } ]
        }"#;
        let def = GuiDef::from_manifest(blank).unwrap();
        assert!(def.validate().unwrap_err().contains("label"));
    }

    #[test]
    fn foreign_namespace_kinds_are_rejected_per_pack() {
        let kind = crate::gui::intern_kind("layouttest:owned").unwrap();
        assert!(kind_permitted(kind, Some("layouttest")).is_ok());
        assert!(kind_permitted(kind, Some("otherpack")).is_err());
        assert!(
            kind_permitted(kind, None).is_err(),
            "base dirs own no namespace"
        );
        // Engine kinds may ship from anywhere (base bakes, re-skin packs).
        assert!(kind_permitted(GuiKind::Furnace, None).is_ok());
        assert!(kind_permitted(GuiKind::Furnace, Some("anypack")).is_ok());
    }

    #[test]
    fn unknown_bare_kind_fails_validation() {
        let json = r#"{ "type": "bogus_kind", "canvas": { "w": 32, "h": 32 }, "scale": 1, "image": "p.png" }"#;
        let def = GuiDef::from_manifest(json).unwrap();
        assert_eq!(def.kind, GuiKind::Other);
        assert!(def.validate().unwrap_err().contains("unknown gui type"));
    }

    #[test]
    fn role_maps_to_the_right_menu_slot() {
        assert_eq!(Role::Storage.menu_slot(5), Some(MenuSlot::Chest(5)));
        assert_eq!(Role::Hotbar.menu_slot(3), Some(MenuSlot::Inventory(3)));
        assert_eq!(
            Role::PlayerInv.menu_slot(0),
            Some(MenuSlot::Inventory(HOTBAR_LEN))
        );
        assert_eq!(
            Role::CraftInput.menu_slot(4),
            Some(MenuSlot::Craft(CraftHit::Input(4)))
        );
        assert_eq!(
            Role::CraftResult.menu_slot(0),
            Some(MenuSlot::Craft(CraftHit::Result))
        );
        assert_eq!(
            Role::FurnaceFuel.menu_slot(0),
            Some(MenuSlot::Furnace(FurnaceHit::Fuel))
        );
        assert_eq!(Role::Generic.menu_slot(0), None);
    }

    #[test]
    fn hit_round_trips_through_slot_center() {
        let def = GuiDef::from_manifest(CHEST).unwrap();
        let screen = (1280, 720);
        let r = def.role_rect(Role::Storage, 1, screen).unwrap();
        let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
        assert_eq!(def.role_at_any(screen, c), Some((Role::Storage, 1)));
        assert_eq!(def.hovered_slot_rect(screen, c), Some(r));
    }

    #[test]
    fn validate_rejects_wrong_slot_count() {
        // CHEST sample has only 2 storage slots, not the required 27.
        let def = GuiDef::from_manifest(CHEST).unwrap();
        assert!(def.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_row_major_storage() {
        let json = r#"{
            "type": "chest", "canvas": { "w": 352, "h": 332 }, "scale": 2, "image": "c.png",
            "slots": [
                { "role": "storage", "x": 52, "y": 26, "w": 32, "h": 32 },
                { "role": "storage", "x": 16, "y": 26, "w": 32, "h": 32 }
            ]
        }"#;
        let def = GuiDef::from_manifest(json).unwrap();
        assert!(check_row_major(Role::Storage, def.role_slots(Role::Storage)).is_err());
    }

    #[test]
    fn malformed_manifest_is_none() {
        assert!(GuiDef::from_manifest("not json").is_none());
    }

    #[test]
    fn baked_manifests_on_disk_all_validate() {
        // The real bakes the game ships must satisfy the role→index contract.
        // (Reads the actual assets dir; this is the contract guard, not table data.)
        for kind in [
            GuiKind::Chest,
            GuiKind::Inventory,
            GuiKind::CraftingTable,
            GuiKind::Furnace,
            GuiKind::Hotbar,
            GuiKind::FurnitureWorkbench,
        ] {
            assert!(def(kind).is_some(), "{kind:?} manifest missing or invalid");
        }
    }
}
