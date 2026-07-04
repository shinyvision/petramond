//! Neutral GUI/container value types and baked layout contracts shared by app,
//! render, and deterministic container mutation.
//!
//! These types name logical GUI identities, hit-tested slots, and immutable view
//! snapshots. They do not own renderer resources or container mutation; baked
//! layout code maps pixels to these slot identities, and the game menu applies
//! them on the tick.

mod kind;
mod layout;
mod shell_skin;

use crate::inventory::{HOTBAR_LEN, TOTAL_SLOTS};
use crate::item::{ItemStack, ItemType};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use kind::GuiKind;
pub(crate) use kind::{intern_kind, intern_str, kind_key, resolve_kind};
pub(crate) use layout::{
    baked_hovers, baked_panels, baked_sprites, def, hit, panel_contains, GuiDef, LabelAlign,
    OverlayMode, SpriteKey, WidgetDef,
};
pub(crate) use shell_skin::{
    baked_shell_hovers, baked_shell_scroll_thumbs, baked_shell_skins, shell_def, ShellDef,
    ShellKind, ShellRole,
};

/// One value of the open GUI session's state map: written by mods on the tick
/// (`GuiStateSet`), read per frame by the renderer for `label` text, `rotimage`
/// angles (radians, `F32`), and mod overlay fractions.
#[derive(Clone, Debug, PartialEq)]
pub enum GuiValue {
    F32(f32),
    I32(i32),
    Str(String),
}

/// The open GUI session's state map. `BTreeMap` for deterministic iteration;
/// snapshotted behind an `Arc` per frame (copy-on-write on the tick side).
pub type GuiStateMap = BTreeMap<String, GuiValue>;

/// A widget's stable id within its GUI manifest (interned — see
/// [`kind::intern_str`]) so [`MenuSlot`] stays `Copy`.
pub type WidgetId = &'static str;

/// An empty shared state map (the per-frame default when no mod GUI is open).
pub(crate) fn empty_gui_state() -> Arc<GuiStateMap> {
    static EMPTY: std::sync::OnceLock<Arc<GuiStateMap>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Arc::new(GuiStateMap::new())).clone()
}

/// A hit-tested crafting slot: an input cell index (`0..cols*cols`, row-major) or
/// the single output result slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CraftHit {
    Input(usize),
    Result,
}

/// A hit-tested furnace role: the smeltable input, the fuel, or the take-only
/// output. One slot each, so these are identified by role, never by position.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FurnaceHit {
    Input,
    Fuel,
    Output,
}

/// A hit-tested furniture-workbench slot: the single input block, or one of the
/// take-only result cells (`0..` row-major, indexing the recipes the placed block
/// offers).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WorkbenchHit {
    Input,
    Result(usize),
}

/// A click hit-tested to a concrete logical slot identity, the unit the App routes
/// through the deterministic container menu on the next tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuSlot {
    /// A main inventory/hotbar slot (the 36-slot grid drawn under every panel).
    Inventory(usize),
    /// A crafting input cell or the result slot of the open craft grid.
    Craft(CraftHit),
    /// A furnace role slot (smeltable input, fuel, or take-only output).
    Furnace(FurnaceHit),
    /// A chest storage slot index.
    Chest(usize),
    /// A furniture-workbench slot: the input block, or a take-only result cell.
    Workbench(WorkbenchHit),
    /// A manifest `button` widget (mod GUIs). Latches like every slot click;
    /// the tick dispatches it to the kind's owning mod as a `gui_click`.
    Widget(WidgetId),
}

/// A manifest slot role. Each role maps to a logical [`MenuSlot`] by in-role
/// index; decorative roles own no menu slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    Generic,
    Storage,
    PlayerInv,
    Hotbar,
    CraftInput,
    CraftResult,
    FurnaceInput,
    FurnaceFuel,
    FurnaceOutput,
    WorkbenchInput,
    WorkbenchResult,
    #[serde(other)]
    Other,
}

impl Role {
    /// Map this role + its in-role index to the logical slot a click resolves to.
    /// `None` for decorative roles, so stray decorative manifest slots can never
    /// route a click.
    pub(crate) fn menu_slot(self, i: usize) -> Option<MenuSlot> {
        Some(match self {
            Role::Storage => MenuSlot::Chest(i),
            Role::Hotbar => MenuSlot::Inventory(i),
            Role::PlayerInv => MenuSlot::Inventory(HOTBAR_LEN + i),
            Role::CraftInput => MenuSlot::Craft(CraftHit::Input(i)),
            Role::CraftResult => MenuSlot::Craft(CraftHit::Result),
            Role::FurnaceInput => MenuSlot::Furnace(FurnaceHit::Input),
            Role::FurnaceFuel => MenuSlot::Furnace(FurnaceHit::Fuel),
            Role::FurnaceOutput => MenuSlot::Furnace(FurnaceHit::Output),
            Role::WorkbenchInput => MenuSlot::Workbench(WorkbenchHit::Input),
            Role::WorkbenchResult => MenuSlot::Workbench(WorkbenchHit::Result(i)),
            Role::Generic | Role::Other => return None,
        })
    }
}

/// One slot's pixel rectangle (interior, where the icon + digits go). All in
/// physical pixels, top-left origin, y down. Shared by GUI drawing and hit testing.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SlotRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl SlotRect {
    /// Whether physical-pixel point `(px, py)` lies within this slot's interior
    /// (half-open: includes the top-left edge, excludes the bottom-right).
    #[inline]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Integer GUI scale chosen from the screen size (vanilla-style auto scale): one
/// step per ~240 logical px of height (and ~320 of width), clamped to `1..=4`.
pub fn gui_scale(screen: (u32, u32)) -> f32 {
    let (w, h) = screen;
    let by_h = (h / 240).max(1);
    let by_w = (w / 320).max(1);
    by_h.min(by_w).clamp(1, 4) as f32
}

pub(crate) const TEXT_GLYPH_W: u32 = 5;
pub(crate) const TEXT_GLYPH_H: u32 = 7;
pub(crate) const TEXT_GLYPH_ADVANCE: u32 = TEXT_GLYPH_W + 1;

#[inline]
pub(crate) fn shell_text_width_chars(chars: usize) -> u32 {
    if chars == 0 {
        0
    } else {
        chars as u32 * TEXT_GLYPH_ADVANCE - 1
    }
}

pub(crate) fn shell_input_text_rect(rect: SlotRect, scale: f32) -> SlotRect {
    let pad = 5.0 * scale;
    SlotRect {
        x: rect.x + pad,
        y: rect.y,
        w: (rect.w - pad * 2.0).max(0.0),
        h: rect.h,
    }
}

pub(crate) fn shell_input_visible_chars(rect: SlotRect, scale: f32) -> usize {
    let cell = scale.max(1.0);
    let text_rect = shell_input_text_rect(rect, scale);
    let units = (text_rect.w / cell).floor();
    if units < TEXT_GLYPH_W as f32 {
        0
    } else {
        ((units + 1.0) / TEXT_GLYPH_ADVANCE as f32).floor() as usize
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum HoverFit {
    Stretch,
    Tile,
    NineSlice {
        src_l: f32,
        src_r: f32,
        src_t: f32,
        src_b: f32,
        dst_l: f32,
        dst_r: f32,
        dst_t: f32,
        dst_b: f32,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum HoverFitJson {
    Stretch,
    Tile,
    NineSlice { l: f32, r: f32, t: f32, b: f32 },
}

impl Default for HoverFitJson {
    fn default() -> Self {
        HoverFitJson::NineSlice {
            l: 4.0,
            r: 4.0,
            t: 4.0,
            b: 4.0,
        }
    }
}

impl HoverFit {
    pub(crate) fn from_json(fit: HoverFitJson, author_scale: f32) -> Self {
        match fit {
            HoverFitJson::Stretch => HoverFit::Stretch,
            HoverFitJson::Tile => HoverFit::Tile,
            HoverFitJson::NineSlice { l, r, t, b } => {
                let s = author_scale.max(1.0);
                HoverFit::NineSlice {
                    src_l: l,
                    src_r: r,
                    src_t: t,
                    src_b: b,
                    dst_l: l / s,
                    dst_r: r / s,
                    dst_t: t / s,
                    dst_b: b / s,
                }
            }
        }
    }
}

/// A furnace's view for the open furnace screen: its three slots plus the two
/// progress gauges (`0.0..=1.0`). `Copy` (`ItemStack` is `Copy`), so render can
/// snapshot it by value with no borrow.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FurnaceView {
    pub input: Option<ItemStack>,
    pub fuel: Option<ItemStack>,
    pub output: Option<ItemStack>,
    /// Smelt progress (drives the arrow): 0 at the start of an item, 1 when done.
    pub cook01: f32,
    /// Remaining fuel of the current burn (drives the flame): 1 full -> 0 spent.
    pub burn01: f32,
}

/// A chest's view for the open chest screen: its 27 storage slots, row-major.
/// `Copy` (`ItemStack` is `Copy`), so render can snapshot it by value with no
/// borrow.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ChestView {
    pub slots: [Option<ItemStack>; crate::chest::CHEST_SLOTS],
}

/// A furniture workbench's view for its open screen: the placed input block and
/// the list of results it offers, each flagged craftable (enough input) or not
/// (shown greyed). The result list is row-major, mapping to the manifest's result
/// slots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkbenchView {
    pub input: Option<ItemStack>,
    /// `(result item, craftable now)` per offered recipe, row-major.
    pub results: Vec<(ItemType, bool)>,
}

/// The player's health for the HUD hearts: `current` and `max` in half-heart points
/// (a full heart is 2). `None` in a [`UiSnapshot`] hides the bar (spectator).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HealthView {
    pub current: i32,
    pub max: i32,
}

/// An owned, neutral UI read model of the flat UI data needed to draw the hotbar
/// or open menu. Built by the app presentation boundary and consumed by the
/// renderer.
#[derive(Clone, Debug)]
pub struct UiSnapshot {
    pub open: bool,
    /// Which baked GUI this frame draws — the open menu's kind, or `Hotbar` for the
    /// HUD. Selects the panel/hover/overlay textures and the slot layout.
    pub kind: GuiKind,
    pub screen: (u32, u32),
    pub cursor_px: (f32, f32),
    pub active: u8,
    /// One entry per inventory slot (`[0,9)` hotbar, `[9,36)` main grid):
    /// `(item, count)`, or `None` for an empty slot.
    pub slots: [Option<(ItemType, u8)>; TOTAL_SLOTS],
    /// The crafting input cells (only the first `panel.cols()²` are drawn).
    pub craft: [Option<(ItemType, u8)>; crate::crafting::MAX_CELLS],
    /// The crafting result preview, drawn in the result slot.
    pub result: Option<(ItemType, u8)>,
    /// The cursor-held stack (drag/drop), drawn at `cursor_px` when open.
    pub cursor: Option<(ItemType, u8)>,
    /// The open furnace's slots + progress gauges, or `None` when the open panel is
    /// not a furnace. When `Some`, the furnace panel is drawn instead of the grid.
    pub furnace: Option<FurnaceView>,
    /// The open chest's 27 storage slots, or `None`. When `Some`, the chest panel +
    /// storage grid are drawn instead of the crafting grid.
    pub chest: Option<ChestView>,
    /// The open furniture workbench's input + offered results, or `None`. When `Some`,
    /// the workbench panel is drawn with the result grid (greyed where not craftable).
    pub workbench: Option<WorkbenchView>,
    /// The player's health for the bottom-left hearts, or `None` to hide the bar
    /// (spectator). Drawn only with the [`GuiKind::Hotbar`] HUD, not behind an open menu.
    pub health: Option<HealthView>,
    /// The open mod GUI's state map (labels / rotimage angles / overlay
    /// fractions), or `None` when no mod GUI session is up. A cheap `Arc`
    /// clone of the tick-owned map.
    pub gui_state: Option<Arc<GuiStateMap>>,
    pub shell: ShellUiSnapshot,
}

impl Default for UiSnapshot {
    fn default() -> Self {
        UiSnapshot {
            open: false,
            kind: GuiKind::Hotbar,
            screen: (0, 0),
            cursor_px: (0.0, 0.0),
            active: 0,
            slots: [None; TOTAL_SLOTS],
            craft: [None; crate::crafting::MAX_CELLS],
            result: None,
            cursor: None,
            furnace: None,
            chest: None,
            workbench: None,
            health: None,
            gui_state: None,
            shell: ShellUiSnapshot::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ShellUiSnapshot {
    pub active: bool,
    pub skin: Option<ShellKind>,
    pub quads: Vec<ShellQuad>,
    pub texts: Vec<ShellText>,
    pub buttons: Vec<ShellButton>,
    pub inputs: Vec<ShellInput>,
    pub rows: Vec<ShellListRow>,
    pub scrollbars: Vec<ShellScrollbar>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShellQuad {
    pub rect: SlotRect,
    pub color: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShellText {
    pub rect: SlotRect,
    pub text: String,
    pub color: [f32; 4],
    pub cell_px: f32,
    pub align: ShellTextAlign,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShellTextAlign {
    Left,
    Center,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShellButton {
    pub rect: SlotRect,
    pub label: String,
    pub enabled: bool,
    pub hovered: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShellInput {
    pub rect: SlotRect,
    pub text: String,
    pub placeholder: String,
    pub active: bool,
    pub cursor: usize,
    pub selection: Option<(usize, usize)>,
    pub show_cursor: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShellListRow {
    pub rect: SlotRect,
    pub label: String,
    pub selected: bool,
    pub hovered: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShellScrollbar {
    pub track: SlotRect,
    pub thumb: SlotRect,
}
