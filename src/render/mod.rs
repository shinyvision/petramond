//! WGPU renderer: atlas texture, opaque + transparent pipelines, fog.

mod block_model;
mod break_overlay;
mod chest_model;
mod crosshair;
mod foliage_tint;
mod hand;
mod item_entity;
mod item_model;
mod lighting;
mod particles;
mod pipeline;
mod renderer;
mod resources;
mod section_cull;
mod selection;
mod ui;
mod ui_text;
mod uniforms;

pub use renderer::{
    instance_descriptor, new_renderer, new_renderer_from_target, new_renderer_with_instance,
    Renderer, UiSnapshot,
};
pub use resources::{GpuMesh, GuiSprite};
pub use uniforms::{
    Uniforms, FOG_END, FOG_START, UNDERWATER_FOG_END, UNDERWATER_FOG_START, UV_RECTS_LEN,
};

pub use block_model::{
    billboard_quad, cube_solid, cube_textured, BillboardBasis, SOLID_COLOR_FLAG,
};

/// Pure UI layout hit-test (contract §9): the inventory slot index under the
/// cursor, or `None`. Shared with the App for drag/drop; uses the same slot-rect
/// math the renderer draws with.
pub use ui::slot_at_cursor;

/// Pure UI layout hit-test: whether the cursor is over the open inventory panel
/// rectangle. Shared with the App to tell a "drop outside the inventory" click
/// (throw the held stack) from a click on the panel itself. Uses the same panel
/// placement the renderer draws with.
pub use ui::cursor_in_panel;

/// Crafting layout kind + the crafting-slot hit-test, shared with the App so a
/// click on a craft input cell / result slot routes to the right action.
pub use ui::{craft_slot_at_cursor, CraftHit, CraftKind};

/// Furnace-slot hit-test (input / fuel / output), shared with the App so a click
/// in the open furnace screen routes to the right slot.
pub use ui::{furnace_slot_at_cursor, FurnaceHit};

/// Chest storage-slot hit-test (the `0..27` slot index under the cursor), shared
/// with the App so a click in the open chest screen routes to the right slot.
pub use ui::chest_slot_at_cursor;

use crate::block::Block;
use crate::item::{ItemStack, ItemType};
use glam::{IVec3, Vec3};

/// The block-break overlay to draw this frame: a cracked-texture quad over
/// `block` at crack `stage` (0..=9, where 9 is fully cracked / about to break).
/// `None` (cleared via [`Renderer::set_break_overlay`]) draws nothing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    /// The block kind at `block`, so a non-full-cube block (the chest) cracks over
    /// its inset visual box instead of the whole cell.
    pub block_kind: Block,
    /// 0..=9 crack stage (maps to `Tile::DestroyStage0..9`).
    pub stage: u8,
}

/// The first-person held item to draw this frame. `item == None` draws the bare
/// skin hand. `swing` (0..1) drives the punch animation (mining and placing
/// both); `swing_scale` (0..1) scales its amplitude so a placement reads as a
/// softer version of the mining punch. The renderer presentation layer owns
/// these visual animation phases.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldItemView {
    pub item: Option<ItemType>,
    /// 0..1 punch phase (sawtooth while mining, one-shot for a break/place).
    pub swing: f32,
    /// Amplitude of the current swing: `1.0` for a mining/break punch, less for
    /// the gentler place jab. Ignored when `swing == 0.0`.
    pub swing_scale: f32,
}

impl Default for HeldItemView {
    fn default() -> Self {
        HeldItemView {
            item: None,
            swing: 0.0,
            swing_scale: 1.0,
        }
    }
}

/// Sim intent for the first-person held item. The renderer consumes this each
/// frame and advances the visual hand/item animation internally.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldItemFrame {
    pub item: Option<ItemType>,
    pub mining: bool,
    /// True on the frame a block breaks, including instant hardness-0 blocks.
    pub broke_block: bool,
    /// True on the frame the hand expels an item into the world — placing a block
    /// or throwing/dropping a stack — which plays the softer place jab.
    pub placed: bool,
    pub dt: f32,
}

/// A dropped item-entity to draw in the world this frame: a small spinning +
/// bobbing cube (or billboard for sprite-kind items) at `pos`, rotated by `spin`
/// radians about Y. The App fills a slice of these from its `DroppedItem`s. A
/// stack draws as several offset, layered copies (capped at 5) per `count`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemEntityInstance {
    pub pos: Vec3,
    pub item: ItemType,
    /// Stack size. Drives how many layered geometries the pile draws (1..=5).
    pub count: u8,
    /// Y-axis spin in radians.
    pub spin: f32,
    /// 6-bit skylight sampled from the world at the dropped item's position.
    pub skylight: u8,
}

/// A placed chest to draw in the world this frame: an inset body box plus a lid
/// hinged open by `lid01` (`0` closed .. `1` fully open), oriented to `facing` at the
/// block `pos` (the block's min corner). The game fills a slice of these from the
/// loaded chunks' chest block-entities; the renderer frustum-culls + bakes them with
/// [`chest_model::build_chests`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ChestInstance {
    /// World position of the block's min corner (block coords as f32).
    pub pos: Vec3,
    /// Placement orientation (which way the front + latch face).
    pub facing: crate::furnace::Facing,
    /// Lid open fraction: `0.0` closed, `1.0` fully open.
    pub lid01: f32,
    /// 6-bit skylight sampled from the world at the chest's cell.
    pub skylight: u8,
}

/// A single particle billboard to draw this frame. `uv_min` / `uv_size` are
/// **absolute** atlas coordinates (sub-tile patch), produced by
/// `crate::entity::Particle::atlas_uv`, so the particle pass samples the block
/// atlas directly with no further tile lookup.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ParticleInstance {
    pub pos: Vec3,
    /// Absolute atlas uv of the patch's min corner.
    pub uv_min: [f32; 2],
    /// Absolute atlas uv extent of the (square) patch.
    pub uv_size: f32,
    /// RGB tint multiplied into the sampled atlas colour (foliage-green for a
    /// grass/leaf fleck, white otherwise), from `crate::entity::Particle::tint`.
    pub tint: [f32; 3],
    pub alpha: f32,
    /// World-space billboard size (side length).
    pub size: f32,
    /// 6-bit skylight sampled from the world at the particle position.
    pub skylight: u8,
}

/// A furnace's view for the open furnace screen: its three slots plus the two
/// progress gauges (`0.0..=1.0`). `Copy` (`ItemStack` is `Copy`), so the renderer
/// snapshots it by value with no borrow.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FurnaceView {
    pub input: Option<ItemStack>,
    pub fuel: Option<ItemStack>,
    pub output: Option<ItemStack>,
    /// Smelt progress (drives the arrow): 0 at the start of an item, 1 when done.
    pub cook01: f32,
    /// Remaining fuel of the current burn (drives the flame): 1 full → 0 spent.
    pub burn01: f32,
}

/// A chest's view for the open chest screen: its 27 storage slots, row-major.
/// `Copy` (`ItemStack` is `Copy`), so the renderer snapshots it by value with no
/// borrow — exactly like [`FurnaceView`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ChestView {
    pub slots: [Option<ItemStack>; crate::chest::CHEST_SLOTS],
}

/// Per-frame UI state handed to the renderer. Borrows the `Inventory` for the
/// duration of the [`Renderer::set_ui`] call only; the renderer snapshots the
/// small bits it needs into owned state so it never holds a borrow across frames.
pub struct UiFrame<'a> {
    pub open: bool,
    /// Which crafting layout the open panel shows (2×2 inventory vs 3×3 table).
    pub panel: CraftKind,
    pub inv: &'a crate::inventory::Inventory,
    /// The active crafting input cells (`len == panel.cols()²`).
    pub craft: &'a [Option<ItemStack>],
    /// The crafting result preview for the result slot.
    pub craft_result: Option<ItemStack>,
    /// The open furnace's slots + gauges, or `None` when the open panel is not a
    /// furnace. When `Some`, the furnace panel replaces the crafting grid.
    pub furnace: Option<FurnaceView>,
    /// The open chest's 27 storage slots, or `None` when the open panel is not a
    /// chest. When `Some`, the chest panel + storage grid replace the crafting grid.
    pub chest: Option<ChestView>,
    /// Screen size in physical pixels `(width, height)`.
    pub screen: (u32, u32),
    /// Cursor position in physical pixels `(x, y)` (for the open-inventory cursor
    /// stack + drag/drop hit-testing).
    pub cursor_px: (f32, f32),
}
