//! Neutral GUI/container value types shared by app, render, and deterministic
//! container mutation.
//!
//! These types name logical GUI identities, slot roles, and immutable view
//! snapshots. They do not own renderer resources or container mutation; the
//! GUI-document runtime ([`documents`]) maps pixels to these slot identities,
//! and the game menu applies them on the tick.

pub(crate) mod doc_theme;
pub(crate) mod documents;
mod kind;

use crate::inventory::{HOTBAR_LEN, TOTAL_SLOTS};
use crate::item::{ItemStack, ItemType};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use kind::GuiKind;
pub(crate) use kind::{intern_kind, intern_str, kind_key, resolve_kind};

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
    /// Resolve a GUI-document role string (the document runtime speaks role
    /// strings; the game owns the mapping to slot identities).
    pub(crate) fn from_key(key: &str) -> Option<Role> {
        Some(match key {
            "storage" => Role::Storage,
            "player_inv" => Role::PlayerInv,
            "hotbar" => Role::Hotbar,
            "craft_input" => Role::CraftInput,
            "craft_result" => Role::CraftResult,
            "furnace_input" => Role::FurnaceInput,
            "furnace_fuel" => Role::FurnaceFuel,
            "furnace_output" => Role::FurnaceOutput,
            "workbench_input" => Role::WorkbenchInput,
            "workbench_result" => Role::WorkbenchResult,
            _ => return None,
        })
    }

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

/// Integer GUI scale chosen from the screen size (vanilla-style auto scale): one
/// step per ~240 logical px of height (and ~320 of width), clamped to `1..=4`.
pub fn gui_scale(screen: (u32, u32)) -> f32 {
    let (w, h) = screen;
    let by_h = (h / 240).max(1);
    let by_w = (w / 320).max(1);
    by_h.min(by_w).clamp(1, 4) as f32
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
    /// Which GUI this frame draws — the open menu's kind, or `Hotbar` for the
    /// HUD.
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
    /// `Some` when a GUI DOCUMENT draws this frame's screen: the document's
    /// solved slot cells (possibly empty). `build_ui` then emits ONLY the
    /// game-owned content (item icons, counts, hearts, the drag stack) into
    /// these rects and skips every legacy panel/shell group — the document
    /// draw list owns all chrome.
    pub doc_slots: Option<Arc<Vec<DocSlot>>>,
}

/// One document slot cell: the game role, in-role index, and physical rect.
#[derive(Clone, Debug, PartialEq)]
pub struct DocSlot {
    pub(crate) role: Role,
    pub(crate) index: u32,
    pub(crate) rect: SlotRect,
}

impl DocSlot {
    pub(crate) fn new(role: Role, index: u32, rect: SlotRect) -> DocSlot {
        DocSlot { role, index, rect }
    }
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
            doc_slots: None,
        }
    }
}
