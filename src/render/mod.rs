//! WGPU renderer: atlas texture, opaque + transparent pipelines, fog.

mod block_model;
mod break_overlay;
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

use crate::item::ItemType;
use glam::{IVec3, Vec3};

/// The block-break overlay to draw this frame: a cracked-texture quad over
/// `block` at crack `stage` (0..=9, where 9 is fully cracked / about to break).
/// `None` (cleared via [`Renderer::set_break_overlay`]) draws nothing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BreakOverlayView {
    pub block: IVec3,
    /// 0..=9 crack stage (maps to `Tile::DestroyStage0..9`).
    pub stage: u8,
}

/// The first-person held item to draw this frame. `item == None` draws the bare
/// skin hand. `swing` (0..1) drives the mining-punch animation; `place_pop`
/// (0..1) drives the one-shot place nudge. The renderer presentation layer owns
/// these visual animation phases.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldItemView {
    pub item: Option<ItemType>,
    /// 0..1 mining punch phase (sawtooth while mining).
    pub swing: f32,
    /// 0..1 one-shot place-pop phase.
    pub place_pop: f32,
}

impl Default for HeldItemView {
    fn default() -> Self {
        HeldItemView {
            item: None,
            swing: 0.0,
            place_pop: 0.0,
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
    pub placed: bool,
    pub dt: f32,
}

/// A dropped item-entity to draw in the world this frame: a small spinning +
/// bobbing cube (or billboard for sprite-kind items) at `pos`, rotated by `spin`
/// radians about Y. The App fills a slice of these from its `DroppedItem`s.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemEntityInstance {
    pub pos: Vec3,
    pub item: ItemType,
    /// Y-axis spin in radians.
    pub spin: f32,
    /// 6-bit skylight sampled from the world at the dropped item's position.
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

/// Per-frame UI state handed to the renderer. Borrows the `Inventory` for the
/// duration of the [`Renderer::set_ui`] call only; the renderer snapshots the
/// small bits it needs into owned state so it never holds a borrow across frames.
pub struct UiFrame<'a> {
    pub open: bool,
    pub inv: &'a crate::inventory::Inventory,
    /// Screen size in physical pixels `(width, height)`.
    pub screen: (u32, u32),
    /// Cursor position in physical pixels `(x, y)` (for the open-inventory cursor
    /// stack + drag/drop hit-testing).
    pub cursor_px: (f32, f32),
}
