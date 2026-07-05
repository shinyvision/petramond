//! WGPU renderer: atlas texture, opaque + transparent pipelines, fog.

mod block_model;
mod break_overlay;
mod chest_model;
mod crosshair;
mod door_model;
mod foliage_tint;
mod hand;
mod hand_animator;
mod item_entity;
mod item_model;
mod lighting;
mod mob_model;
mod particles;
mod pipeline;
mod renderer;
mod resources;
mod scene;
mod selection;
mod shader_pack;
mod ui;
mod ui_text;
mod uniforms;

pub use crate::game::presentation::BreakOverlayView;
pub(crate) use renderer::new_renderer_from_target;
pub use renderer::Renderer;

pub(crate) use scene::Scene;

#[cfg(test)]
pub use block_model::SOLID_COLOR_FLAG;

use crate::block_state::HeldBlockState;
use crate::item::ItemType;
use glam::{Quat, Vec3};
use std::sync::Arc;

/// The first-person held item to draw this frame. `item == None` draws the bare
/// skin hand. `swing` (0..1) drives the punch animation (mining and placing
/// both); `swing_scale` (0..1) scales its amplitude so a placement reads as a
/// softer version of the mining punch. The renderer presentation layer owns
/// these visual animation phases.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldItemView {
    pub item: Option<ItemType>,
    pub block_state: HeldBlockState,
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
            block_state: HeldBlockState::None,
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
    pub block_state: HeldBlockState,
    pub mining: bool,
    /// True on the frame a block breaks, including instant hardness-0 blocks.
    pub broke_block: bool,
    /// True on the frame the hand expels an item into the world — placing a block
    /// or throwing/dropping a stack — which plays the softer place jab.
    pub placed: bool,
    /// True on the frame the player swings to attack — a mob hit or a punch at the
    /// air — which plays a full-strength one-shot swing (like a block break).
    pub swung: bool,
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
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    pub blocklight: u8,
}

/// One animated mob to draw in the world this frame: a species (`kind`) posed at
/// `anim_time` into its walk cycle (when `moving`; otherwise its rest pose), placed
/// at `pos` (its feet) facing `yaw`, lit by the sampled `skylight`. The scene
/// adapter fills a slice of these by interpolating the sim's live mob instances; the
/// renderer groups them by species, frustum-culls, and bakes each with
/// [`mob_model::build_mob_instances`] against that species' model + texture.
#[derive(Clone, Debug, PartialEq)]
pub struct MobRenderInstance {
    /// Which species (selects the model / texture / draw buffers).
    pub kind: crate::mob::Mob,
    /// World position of the mob's feet (model `y=0`).
    pub pos: Vec3,
    /// Facing yaw in radians (rotation about Y).
    pub yaw: f32,
    /// Seconds into the active animation (walk or idle_*); used unless idle+resting.
    pub anim_time: f32,
    /// Whether the mob is walking this frame: plays the walk animation if so.
    pub moving: bool,
    /// When idle, which `idle_*` animation is playing (index), or `None` for the
    /// neutral rest pose.
    pub idle_anim: Option<u8>,
    /// Head orientation relative to the body (radians): yaw swivel, pitch tilt.
    /// Applied to the model's `head` bone unless the active animation moves the head.
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// 6-bit skylight sampled from the world at the mob's position.
    pub skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    pub blocklight: u8,
    /// Hurt-flash intensity in `[0, 1]`: tints the mob red after a non-lethal hit,
    /// fading out. `0` for an unhurt or dead mob.
    pub hurt: f32,
    /// Whether the mob is currently shorn: the bake skips the model's coat cubes
    /// (the ones named `wool`) so the fleece disappears until it regrows.
    pub shorn: bool,
    /// When the mob is dying, its per-bone ragdoll pose — `(rest-pivot position,
    /// rotation delta)` per bone in model space, already interpolated for this frame —
    /// used over the authored rest pose. `None` for a live mob. `Arc` so cloning a
    /// visible instance into its per-species batch stays cheap.
    pub ragdoll: Option<Arc<[(Vec3, Quat)]>>,
}

/// A placed chest to draw in the world this frame: an inset body box plus a lid
/// hinged open by `lid01` (`0` closed .. `1` fully open), oriented to `facing` at the
/// block `pos` (the block's min corner). The game fills a slice of these from the
/// loaded chunks' chest block-entities; the renderer frustum-culls + bakes them with
/// [`chest_model::build_chests`].
#[derive(Copy, Clone, Debug, PartialEq)]
struct ChestInstance {
    /// World position of the block's min corner (block coords as f32).
    pos: Vec3,
    /// Placement orientation (which way the front + latch face).
    facing: crate::furnace::Facing,
    /// Lid open fraction: `0.0` closed, `1.0` fully open.
    lid01: f32,
    /// 6-bit skylight sampled from the world at the chest's cell.
    skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    blocklight: u8,
}

/// A placed door to draw in the world this frame: a 2-tall thin slab on the `facing`
/// edge of cell `pos` (the lower cell's min corner), swung open by `open01`
/// (`0` closed .. `1` fully open). The game fills a slice of these from the loaded
/// chunks' door state; the renderer frustum-culls + bakes them with
/// [`door_model::build_doors`]. The two halves carry different art (`bottom_tile` /
/// `top_tile`).
#[derive(Copy, Clone, Debug, PartialEq)]
struct DoorInstance {
    /// World position of the lower cell's min corner (block coords as f32).
    pos: Vec3,
    /// The edge the CLOSED door rests on (its outward normal); see [`crate::door`].
    facing: crate::furnace::Facing,
    /// Swing fraction: `0.0` closed, `1.0` fully open onto the adjacent edge.
    open01: f32,
    /// Atlas tile for the lower half's front/back (door art).
    bottom_tile: crate::atlas::Tile,
    /// Atlas tile for the upper half's front/back (door art).
    top_tile: crate::atlas::Tile,
    /// Atlas tile for the four thin EDGE faces (the door's side — distinct from the
    /// front art, e.g. a plank strip).
    side_tile: crate::atlas::Tile,
    /// 6-bit skylight sampled from the world at the door's lower cell.
    skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    blocklight: u8,
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
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    pub blocklight: u8,
}
