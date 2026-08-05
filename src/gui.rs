//! Neutral GUI/container value types shared by app, render, and deterministic
//! container mutation.
//!
//! These types name logical GUI identities, slot roles, and immutable view
//! snapshots. They do not own renderer resources or container mutation; the
//! GUI-document runtime ([`documents`]) maps pixels to these slot identities,
//! and the game menu applies them on the tick.

pub mod doc_theme;
pub mod documents;

use petramond_world::inventory::{HOTBAR_LEN, TOTAL_SLOTS};
use petramond_world::item::{ItemStack, ItemType};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

// Compat re-exports while callers migrate to `gui_state` (the shared session
// vocabulary now lives below the GUI machinery).
pub use petramond_world::gui_state::{ContainerView, GuiKind, GuiStateMap, HealthView, MenuSlot};
#[allow(unused_imports)]
pub use petramond_world::gui_state::{
    empty_gui_state, gui_state_clear, gui_state_set, intern_kind, intern_str, kind_key,
    resolve_kind, MAX_MENU_DRAG_SLOTS,
};

/// One document image source for the renderer. Static pack art is cached by
/// path; client-WASM rasters are replaced by `(key, revision)` without ever
/// granting the module filesystem or GPU access.
#[derive(Clone)]
pub enum DocImageSource {
    Path(PathBuf),
    Dynamic {
        key: String,
        size: (u32, u32),
        revision: u64,
        rgba: Arc<[u8]>,
    },
}

/// A manifest slot role. Each role maps to a logical [`MenuSlot`] by in-role
/// index; decorative roles own no menu slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Generic,
    PlayerInv,
    Hotbar,
    CraftResult,
    /// ANY container's slots (role string `container`) — the engine chest and
    /// furnace and a pack's own container alike. What each index means is the
    /// document's `SlotSpec` (filters, take-only), never the role name.
    Container,
    #[serde(other)]
    Other,
}

impl Role {
    /// Resolve a GUI-document role string (the document runtime speaks role
    /// strings; the game owns the mapping to slot identities).
    pub fn from_key(key: &str) -> Option<Role> {
        Some(match key {
            "player_inv" => Role::PlayerInv,
            "hotbar" => Role::Hotbar,
            "craft_result" => Role::CraftResult,
            "container" => Role::Container,
            _ => return None,
        })
    }

    /// Map this role + its in-role index to the logical slot a click resolves to.
    /// `None` for decorative roles, so stray decorative manifest slots can never
    /// route a click.
    pub fn menu_slot(self, i: usize) -> Option<MenuSlot> {
        Some(match self {
            Role::Hotbar => MenuSlot::Inventory(i),
            Role::PlayerInv => MenuSlot::Inventory(HOTBAR_LEN + i),
            Role::CraftResult => MenuSlot::CraftResult,
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

/// A game-owned item-icon region embedded in a GUI document. Grid-cell hooks
/// carry their filtered row index; the detail and tooltip hooks describe one
/// recipe the browser named in the snapshot, so they carry none.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DocHookKind {
    /// One recipe-grid cell's result icon (`index` = filtered row).
    RecipeResult,
    /// The hovered recipe's result icon / ingredient strip, in the tooltip.
    TipResult,
    TipIngredients,
    /// A document `hook` with an `item` binding: draw this item's icon scaled
    /// into the hook's rect — the generic "show an item here" view (a mod
    /// panel's enlarged subject). The bound value is an ordered comma-LIST of
    /// registry names; each resolved name becomes one `ItemView` hook at the
    /// same rect, in list order, so later names composite over earlier ones
    /// (the anvil's tool + augment overlays).
    ItemView(petramond_world::item::ItemType),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DocHook {
    pub kind: DocHookKind,
    pub index: usize,
    pub rect: SlotRect,
    pub clip: Option<SlotRect>,
    /// The hook floats in a tooltip: its content draws in the overlay tier,
    /// after the base tier's icons, so the grid never shows through it.
    pub overlay: bool,
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
pub struct UiViewport {
    pub size: (u32, u32),
    pub scale: i32,
    pub generation: u64,
}

impl UiViewport {
    pub fn new(size: (u32, u32), generation: u64) -> UiViewport {
        UiViewport {
            size,
            scale: gui_scale(size) as i32,
            generation,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn unversioned(size: (u32, u32)) -> UiViewport {
        UiViewport::new(size, 0)
    }
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
    /// One entry per inventory slot (`[0,9)` hotbar, `[9,36)` main grid);
    /// `None` for an empty slot.
    pub slots: [Option<ItemStack>; TOTAL_SLOTS],
    /// The real player-crafting output, drawn in the take-only result slot.
    pub craft_output: Option<ItemStack>,
    /// Filtered recipe rows, in the same order as the browser document list.
    pub craft_recipes: Vec<CraftingRecipeView>,
    /// The recipe under the cursor, shown in the floating tooltip.
    pub craft_tip: Option<CraftingRecipeView>,
    /// The cursor-held stack (drag/drop), drawn at `cursor_px` when open.
    pub cursor: Option<ItemStack>,
    /// The open container's slots, or `None` when the open session has none.
    pub container: Option<ContainerView>,
    /// The player's health for the bottom-left hearts, or `None` to hide the bar
    /// (spectator). Drawn only with the [`GuiKind::Hotbar`] HUD, not behind an open menu.
    pub health: Option<HealthView>,
    /// The player's active status effects, in application order — the framed
    /// icon row drawn directly above the hearts (hidden with them). Empty for
    /// a spectator or when nothing is active.
    pub effects: Vec<petramond_world::effect::Effect>,
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
    pub role: Role,
    pub index: u32,
    pub rect: SlotRect,
    /// The slot's cell paints in the document's RAISED (overlay) tier
    /// (`"overlay": true` on a node), so its stack icon must draw in the
    /// host's overlay content tier too or it would sink under its own cell
    /// face. Generic and tested; currently no shipping document raises a
    /// slot (built for a floated-slot anvil design later dropped).
    pub raised: bool,
}

impl DocSlot {
    pub fn new(role: Role, index: u32, rect: SlotRect) -> DocSlot {
        DocSlot {
            role,
            index,
            rect,
            raised: false,
        }
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
            craft_tip: None,
            cursor: None,
            container: None,
            health: None,
            effects: Vec::new(),
            gui_state: None,
            hurt_flash: 0.0,
            heart_wiggle: None,
        }
    }
}
