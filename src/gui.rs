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
use std::path::PathBuf;
use std::sync::Arc;

pub use kind::GuiKind;
pub(crate) use kind::{intern_kind, intern_str, kind_key, resolve_kind};

/// Maximum distinct destination cells one pointer gesture can ship. The
/// largest supported menu is a 54-slot generic container plus all 36 player
/// inventory slots.
pub(crate) const MAX_MENU_DRAG_SLOTS: usize =
    crate::container::MAX_CONTAINER_SLOTS + crate::inventory::TOTAL_SLOTS;

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

/// One document image source for the renderer. Static pack art is cached by
/// path; client-WASM rasters are replaced by `(key, revision)` without ever
/// granting the module filesystem or GPU access.
#[derive(Clone)]
pub(crate) enum DocImageSource {
    Path(PathBuf),
    Dynamic {
        key: String,
        size: (u32, u32),
        revision: u64,
        rgba: Arc<[u8]>,
    },
}

/// A widget's stable id within its GUI manifest (interned — see
/// [`kind::intern_str`]) so [`MenuSlot`] stays `Copy`.
pub type WidgetId = &'static str;

/// An empty shared state map (the per-frame default when no mod GUI is open).
pub(crate) fn empty_gui_state() -> Arc<GuiStateMap> {
    static EMPTY: std::sync::OnceLock<Arc<GuiStateMap>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Arc::new(GuiStateMap::new())).clone()
}

/// Write a session state key (tick-side; copy-on-write against any
/// outstanding snapshot — at most one clone per snapshot taken). The map lives
/// per player session (`ConnectedPlayer::gui_state`).
pub(crate) fn gui_state_set(map: &mut Arc<GuiStateMap>, key: String, value: GuiValue) {
    Arc::make_mut(map).insert(key, value);
}

/// Reset a session state map for a fresh GUI session (the menu funnels call
/// this on open AND close, so a session can never read a predecessor's
/// values).
pub(crate) fn gui_state_clear(map: &mut Arc<GuiStateMap>) {
    if !map.is_empty() {
        *map = empty_gui_state();
    }
}

/// A hit-tested furnace role: the smeltable input, the fuel, or the take-only
/// output. One slot each, so these are identified by role, never by position.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FurnaceHit {
    Input,
    Fuel,
    Output,
}

/// A click hit-tested to a concrete logical slot identity, the unit the App routes
/// through the deterministic container menu on the next tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuSlot {
    /// A main inventory/hotbar slot (the 36-slot grid drawn under every panel).
    Inventory(usize),
    /// The real, take-only output of an accepted player-crafting request.
    CraftResult,
    /// A furnace role slot (smeltable input, fuel, or take-only output).
    Furnace(FurnaceHit),
    /// A chest storage slot index.
    Chest(usize),
    /// A mod GUI's `container` role slot index, backed by the
    /// [`Container`](crate::container::Container) at the session's
    /// opening block. Slot semantics (filters, take-only) come from the
    /// document's slot nodes.
    Container(usize),
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
    CraftResult,
    FurnaceInput,
    FurnaceFuel,
    FurnaceOutput,
    /// A mod GUI's generic container slots (role string `container`).
    Container,
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
            "craft_result" => Role::CraftResult,
            "furnace_input" => Role::FurnaceInput,
            "furnace_fuel" => Role::FurnaceFuel,
            "furnace_output" => Role::FurnaceOutput,
            "container" => Role::Container,
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
            Role::CraftResult => MenuSlot::CraftResult,
            Role::FurnaceInput => MenuSlot::Furnace(FurnaceHit::Input),
            Role::FurnaceFuel => MenuSlot::Furnace(FurnaceHit::Fuel),
            Role::FurnaceOutput => MenuSlot::Furnace(FurnaceHit::Output),
            Role::Container => MenuSlot::Container(i),
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

/// A game-owned item-icon region embedded in a GUI document. Recipe-browser
/// hooks carry their filtered row index; `clip` preserves scroll clipping.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DocHookKind {
    CraftRecipeResult,
    CraftRecipeIngredients,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DocHook {
    pub(crate) kind: DocHookKind,
    pub(crate) index: usize,
    pub(crate) rect: SlotRect,
    pub(crate) clip: Option<SlotRect>,
}

/// Integer GUI scale chosen from the screen size (vanilla-style auto scale): one
/// step per ~240 logical px of height (and ~320 of width), clamped to `1..=4`.
pub fn gui_scale(screen: (u32, u32)) -> f32 {
    let (w, h) = screen;
    let by_h = (h / 240).max(1);
    let by_w = (w / 320).max(1);
    by_h.min(by_w).clamp(1, 4) as f32
}

/// The one physical viewport authority for a complete UI frame. `generation`
/// changes whenever the renderer reconfigures its surface, so layout produced
/// before a resize can never be combined with geometry produced after it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct UiViewport {
    pub(crate) size: (u32, u32),
    pub(crate) scale: i32,
    pub(crate) generation: u64,
}

impl UiViewport {
    pub(crate) fn new(size: (u32, u32), generation: u64) -> UiViewport {
        UiViewport {
            size,
            scale: gui_scale(size) as i32,
            generation,
        }
    }

    #[cfg(test)]
    pub(crate) fn unversioned(size: (u32, u32)) -> UiViewport {
        UiViewport::new(size, 0)
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
    pub slots: [Option<ItemStack>; crate::world::chest::CHEST_SLOTS],
}

/// An open mod container's view: its slots row-major in document order (the
/// in-role `container` index). Owned (slot counts vary per document), rebuilt
/// per frame from the world like the chest view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerView {
    pub slots: Vec<Option<ItemStack>>,
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
    pub cursor_px: (f32, f32),
    pub active: u8,
    /// One entry per inventory slot (`[0,9)` hotbar, `[9,36)` main grid):
    /// `(item, count)`, or `None` for an empty slot.
    pub slots: [Option<(ItemType, u8)>; TOTAL_SLOTS],
    /// The real player-crafting output, drawn in the take-only result slot.
    pub craft_output: Option<(ItemType, u8)>,
    /// Filtered recipe rows, in the same order as the browser document list.
    pub craft_recipes: Vec<CraftingRecipeView>,
    /// The cursor-held stack (drag/drop), drawn at `cursor_px` when open.
    pub cursor: Option<(ItemType, u8)>,
    /// The open furnace's slots + progress gauges, or `None` when the open panel is
    /// not a furnace. When `Some`, the furnace panel is drawn instead of the grid.
    pub furnace: Option<FurnaceView>,
    /// The open chest's 27 storage slots, or `None`.
    pub chest: Option<ChestView>,
    /// The open mod GUI's container slots, or `None` when the open session is
    /// not a slot-bearing mod GUI.
    pub container: Option<ContainerView>,
    /// The player's health for the bottom-left hearts, or `None` to hide the bar
    /// (spectator). Drawn only with the [`GuiKind::Hotbar`] HUD, not behind an open menu.
    pub health: Option<HealthView>,
    /// The player's active status effects, in application order — the framed
    /// icon row drawn directly above the hearts (hidden with them). Empty for
    /// a spectator or when nothing is active.
    pub effects: Vec<crate::effect::Effect>,
    /// The open mod GUI's state map (labels / rotimage angles / overlay
    /// fractions), or `None` when no mod GUI session is up. A cheap `Arc`
    /// clone of the tick-owned map.
    pub gui_state: Option<Arc<GuiStateMap>>,
    /// Hurt-flash strength in `[0, 1]`: `build_ui` draws a subtle red edge
    /// vignette scaled by it. `0.0` = none (the common frame).
    pub hurt_flash: f32,
    /// An active heart-wiggle burst, or `None`: `(lo, hi, t)` — hearts
    /// overlapping the half-heart point range `[lo, hi)` (the points a heal
    /// added or a hit removed) shake, `t` seconds (wall clock) into the burst.
    /// Set by the app presentation layer; the sim knows nothing of it.
    pub heart_wiggle: Option<(i32, i32, f32)>,
}

/// Host-drawn item data for one filtered recipe-browser row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftingRecipeView {
    pub result: ItemType,
    pub ingredients: Vec<(ItemType, u16)>,
    pub craftable: bool,
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
            cursor_px: (0.0, 0.0),
            active: 0,
            slots: [None; TOTAL_SLOTS],
            craft_output: None,
            craft_recipes: Vec::new(),
            cursor: None,
            furnace: None,
            chest: None,
            container: None,
            health: None,
            effects: Vec::new(),
            gui_state: None,
            hurt_flash: 0.0,
            heart_wiggle: None,
        }
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    /// The GUI state session contract (one state map per player session):
    /// set/get round-trips, clear resets to the shared empty map, and a held
    /// snapshot is a refcount bump that tick-side writes never mutate in place
    /// (copy-on-write) — which is also what makes the menu-sync `Arc`
    /// identity change detection sound.
    #[test]
    fn gui_state_set_get_clear_and_snapshot_cow() {
        let mut map = empty_gui_state();
        assert!(map.get("wheel:angle").is_none());

        gui_state_set(&mut map, "wheel:angle".into(), GuiValue::F32(1.5));
        assert_eq!(map.get("wheel:angle"), Some(&GuiValue::F32(1.5)));

        // A held snapshot keeps its values across later writes, and the write
        // lands on a FRESH allocation (identity change = "changed" on the wire).
        let snap = map.clone();
        gui_state_set(&mut map, "wheel:angle".into(), GuiValue::F32(2.0));
        gui_state_set(
            &mut map,
            "wheel:result".into(),
            GuiValue::Str("stick".into()),
        );
        assert_eq!(snap.get("wheel:angle"), Some(&GuiValue::F32(1.5)));
        assert_eq!(snap.get("wheel:result"), None);
        assert_eq!(map.get("wheel:angle"), Some(&GuiValue::F32(2.0)));
        assert!(
            !Arc::ptr_eq(&snap, &map),
            "a write under a snapshot re-allocates"
        );

        // Unchanged between snapshots = the same allocation (no per-frame copy).
        let a = map.clone();
        let b = map.clone();
        assert!(Arc::ptr_eq(&a, &b));

        gui_state_clear(&mut map);
        assert!(map.get("wheel:angle").is_none());
        assert!(map.get("wheel:result").is_none());
    }
}
