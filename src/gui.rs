pub mod doc_theme;
pub mod documents;

use petramond_world::inventory::{HOTBAR_LEN, TOTAL_SLOTS};
use petramond_world::item::{ItemStack, ItemType};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

#[allow(unused_imports)]
pub use petramond_world::gui_state::{
    empty_gui_state, gui_state_clear, gui_state_set, intern_kind, intern_str, kind_key,
    resolve_kind, MAX_MENU_DRAG_SLOTS,
};
pub use petramond_world::gui_state::{ContainerView, GuiKind, GuiStateMap, HealthView, MenuSlot};

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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Generic,
    PlayerInv,
    Hotbar,
    OffHand,
    CraftResult,
    Container,
    #[serde(other)]
    Other,
}

impl Role {
    pub fn from_key(key: &str) -> Option<Role> {
        Some(match key {
            "player_inv" => Role::PlayerInv,
            "hotbar" => Role::Hotbar,
            "off_hand" => Role::OffHand,
            "craft_result" => Role::CraftResult,
            "container" => Role::Container,
            _ => return None,
        })
    }

    pub fn menu_slot(self, i: usize) -> Option<MenuSlot> {
        Some(match self {
            Role::Hotbar => MenuSlot::Inventory(i),
            Role::PlayerInv => MenuSlot::Inventory(HOTBAR_LEN + i),
            Role::OffHand => MenuSlot::OffHand,
            Role::CraftResult => MenuSlot::CraftResult,
            Role::Container => MenuSlot::Container(i),
            Role::Generic | Role::Other => return None,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SlotRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DocHookKind {
    RecipeResult,
    TipResult,
    TipIngredients,
    ItemView {
        item: petramond_world::item::ItemType,
        dim: bool,
    },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DocHook {
    pub kind: DocHookKind,
    pub index: usize,
    pub rect: SlotRect,
    pub clip: Option<SlotRect>,
    pub overlay: bool,
}

pub fn gui_scale(screen: (u32, u32)) -> f32 {
    let (w, h) = screen;
    let by_h = (h / 240).max(1);
    let by_w = (w / 320).max(1);
    by_h.min(by_w).clamp(1, 4) as f32
}

pub fn frame_ui_scale(frame: (u32, u32)) -> f32 {
    let (w, h) = frame;
    (h / 240).min(w / 320).max(1) as f32
}

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

    pub fn for_frame(size: (u32, u32), generation: u64) -> UiViewport {
        UiViewport {
            size,
            scale: frame_ui_scale(size) as i32,
            generation,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn unversioned(size: (u32, u32)) -> UiViewport {
        UiViewport::new(size, 0)
    }
}

#[derive(Clone, Debug)]
pub struct UiSnapshot {
    pub open: bool,
    pub kind: GuiKind,
    pub cursor_px: (f32, f32),
    pub active: u8,
    pub slots: [Option<ItemStack>; TOTAL_SLOTS],
    pub off_hand: Option<ItemStack>,
    pub craft_output: Option<ItemStack>,
    pub craft_recipes: Vec<CraftingRecipeView>,
    pub craft_tip: Option<CraftingRecipeView>,
    pub cursor: Option<ItemStack>,
    pub container: Option<ContainerView>,
    pub health: Option<HealthView>,
    pub effects: Vec<petramond_world::effect::Effect>,
    pub gui_state: Option<Arc<GuiStateMap>>,
    pub hurt_flash: f32,
    pub heart_wiggle: Option<(i32, i32, f32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftingRecipeView {
    pub result: ItemType,
    pub ingredients: Vec<(ItemType, u16)>,
    pub craftable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocSlot {
    pub role: Role,
    pub index: u32,
    pub rect: SlotRect,
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
            off_hand: None,
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
