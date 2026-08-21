//! WGPU renderer: atlas texture, opaque + transparent pipelines, fog.

#![allow(clippy::too_many_arguments)]

pub mod atlas;
pub mod camera;
pub mod views;
pub mod block_draw;
pub mod break_overlay;
pub mod chest_model;
pub mod crosshair;
pub mod door_model;
pub mod effect_icons;
pub mod entity_shadow;
pub mod foliage_tint;
pub mod geometry_arena;
pub mod gpu_mem;
pub mod gpu_timer;
pub mod hand;
pub mod hand_animator;
pub mod item_cube;
pub mod item_entity;
pub mod item_model;

pub mod lighting;
pub mod mob_model;
pub mod particles;
pub mod pipeline;
pub mod player_model;
pub mod renderer;
pub mod resources;
pub mod scene;
pub mod selection;
pub mod shader_pack;
pub mod ui;
pub mod uniforms;

pub use views::BreakOverlayView;
pub use views::EntityShadow;
pub use hand_animator::HeldItemAnimator;
pub use renderer::new_offscreen_renderer;
pub use renderer::new_renderer_from_target;
#[allow(unused_imports)]
pub use renderer::TerrainMemory;
pub use renderer::{RenderedFrame, Renderer};

pub use scene::Scene;

#[cfg(test)]
pub use item_cube::SOLID_COLOR_FLAG;

use petramond_world::block_state::HeldBlockState;
use petramond_world::item::ItemType;
use glam::{Quat, Vec3};
use std::sync::Arc;

/// One client-WASM image placed in an explicit physical screen rect. This is
/// presentation canvas data, not a GUI document, so GUI scale never applies.
#[derive(Clone)]
pub struct ClientOverlayImage {
    pub key: String,
    pub size: (u16, u16),
    pub rgba: Arc<[u8]>,
    pub revision: u64,
    /// Recent partial updates (see `ClientImageData::recent_blits`): lets the
    /// upload cache refresh only the changed rects when its held revision is
    /// still inside the window.
    pub recent_blits: Vec<(u64, [u16; 4])>,
    pub rect: [f32; 4],
    pub uv: [f32; 4],
}

/// One solved GUI document. Its chrome, slot geometry, and image table are a
/// single stamped unit so none can be paired with another layout generation.
pub struct DocumentUiFrame<'a> {
    pub viewport: petramond::gui::UiViewport,
    pub kind: petramond_world::gui_state::GuiKind,
    pub draw: &'a petramond_ui::DrawList,
    pub images: &'a [petramond::gui::DocImageSource],
    pub slots: &'a [petramond::gui::DocSlot],
    pub hooks: &'a [petramond::gui::DocHook],
}

/// The complete UI handoff for one render frame. Every physical-pixel layer
/// consumes `viewport`; the renderer accepts or rejects this packet as a whole.
pub struct UiFrame<'a> {
    pub viewport: petramond::gui::UiViewport,
    pub document: Option<DocumentUiFrame<'a>>,
    pub content: &'a petramond::gui::UiSnapshot,
    pub client_overlays: &'a [ClientOverlayImage],
    pub client_overlay_dim: bool,
}

impl UiFrame<'_> {
    pub fn matches_viewport(&self, current: petramond::gui::UiViewport) -> bool {
        self.viewport == current
            && self.document.as_ref().is_none_or(|document| {
                document.viewport == self.viewport && document.kind == self.content.kind
            })
    }
}

#[cfg(test)]
mod ui_frame_coherence_tests {
    use super::*;

    #[test]
    fn a_ui_packet_accepts_only_its_complete_viewport_generation_and_kind() {
        let viewport = petramond::gui::UiViewport::new((1280, 720), 7);
        let draw = petramond_ui::DrawList::default();
        let images: Vec<petramond::gui::DocImageSource> = Vec::new();
        let slots = Vec::new();
        let content = petramond::gui::UiSnapshot {
            kind: petramond_world::gui_state::GuiKind::Hotbar,
            ..Default::default()
        };
        let frame = UiFrame {
            viewport,
            document: Some(DocumentUiFrame {
                viewport,
                kind: petramond_world::gui_state::GuiKind::Hotbar,
                draw: &draw,
                images: &images,
                slots: &slots,
                hooks: &[],
            }),
            content: &content,
            client_overlays: &[],
            client_overlay_dim: false,
        };

        assert!(frame.matches_viewport(viewport));
        assert!(!frame.matches_viewport(petramond::gui::UiViewport::new((1280, 720), 8)));

        let stale_document = UiFrame {
            viewport,
            document: Some(DocumentUiFrame {
                viewport: petramond::gui::UiViewport::new((1280, 720), 6),
                kind: petramond_world::gui_state::GuiKind::Hotbar,
                draw: &draw,
                images: &images,
                slots: &slots,
                hooks: &[],
            }),
            content: &content,
            client_overlays: &[],
            client_overlay_dim: false,
        };
        assert!(!stale_document.matches_viewport(viewport));

        let wrong_kind = petramond::gui::UiSnapshot {
            kind: petramond_world::gui_state::GuiKind::Inventory,
            ..Default::default()
        };
        let frame = UiFrame {
            content: &wrong_kind,
            ..frame
        };
        assert!(!frame.matches_viewport(viewport));
    }
}

/// The first-person held item to draw this frame. `item == None` draws the bare
/// skin hand. `swing` (0..1) drives the punch animation (mining and placing
/// both); `swing_scale` (0..1) scales its amplitude so a placement reads as a
/// softer version of the mining punch. The renderer presentation layer owns
/// these visual animation phases.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldItemView {
    pub item: Option<ItemType>,
    /// The held stack's instance-data variant (tint resolution at draw).
    pub variant: petramond_world::item::VariantId,
    pub block_state: HeldBlockState,
    /// The hand's own sway: the camera's bob run through a first-order lag, so
    /// the arm trails the body instead of moving with it (see
    /// `HeldItemAnimator`). View-space offset, already scaled.
    pub bob: [f32; 2],
    /// 0..1 punch phase (sawtooth while mining, one-shot for a break/place).
    pub swing: f32,
    /// Amplitude of the current swing: `1.0` for a mining/break punch, less for
    /// the gentler place jab. Ignored when `swing == 0.0`.
    pub swing_scale: f32,
    /// 0..1 EAT pose blend: how far the held food is carried from its rest
    /// anchor up to the mouth (eased in at eat start, back out on finish or
    /// abort). `0.0` on ordinary frames.
    pub eat: f32,
    /// Signed nibble oscillator (−1..1) while eating — the bite rhythm layered
    /// over the mouth carry. Consumers scale it by [`eat`](Self::eat).
    pub eat_bob: f32,
    /// 0..1 smoothed EAT PROGRESS: while the food wiggles at the mouth, it
    /// slowly closes the remaining DEPTH toward the camera as this rises — the
    /// bite-by-bite approach. Screen-position carry stays on [`eat`](Self::eat).
    pub eat_near: f32,
}

impl Default for HeldItemView {
    fn default() -> Self {
        HeldItemView {
            item: None,
            variant: petramond_world::item::VariantId::NONE,
            block_state: HeldBlockState::None,
            bob: [0.0, 0.0],
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        }
    }
}

/// Sim intent for the first-person held item. The renderer consumes this each
/// frame and advances the visual hand/item animation internally.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldItemFrame {
    pub item: Option<ItemType>,
    /// The held stack's instance-data variant (tint resolution at draw).
    pub variant: petramond_world::item::VariantId,
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
    /// Level: a food item is mid-eat, carrying the eat's progress in `[0, 1)`.
    /// The animator raises the food quickly at the start, then drifts it the
    /// rest of the way to the mouth as the progress advances.
    pub eating: Option<f32>,
    /// The camera's normalized walk sway this frame (`side`, `up` — see
    /// `game::view_bob`). The hand does NOT wear it directly: the animator
    /// lags it, which is what stops the item riding the screen rigidly.
    pub bob: [f32; 2],
    pub dt: f32,
}

/// A dropped item-entity to draw in the world this frame: a small spinning +
/// bobbing cube (or extruded 3D slab for sprite-kind items) at `pos`, rotated by
/// `spin` radians about Y. The App fills a slice of these from its `DroppedItem`s.
/// A stack draws as several offset, layered copies (capped at 5) per `count`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemEntityInstance {
    pub pos: Vec3,
    pub item: ItemType,
    /// The stack's instance-data variant (tint resolution at draw).
    pub variant: petramond_world::item::VariantId,
    /// Stack size. Drives how many layered geometries the pile draws (1..=5).
    pub count: u8,
    /// Y-axis spin in radians.
    pub spin: f32,
    /// 6-bit skylight sampled from the world at the dropped item's position.
    pub skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    pub blocklight: petramond_world::light::BlockLight6,
}

/// One placed block's mod DRAW SET to draw this frame. Owned by the gather
/// that produces it (`world::draw`) and carried here UNCHANGED — see its doc.
pub use petramond::world::draw::BlockDrawInstance;

/// One animated mob to draw in the world this frame: a species (`kind`) posed at
/// `anim_time` into its walk cycle (when `moving`; otherwise its rest pose), placed
/// at `pos` (its feet) facing `yaw`, lit by the sampled `skylight`. The scene
/// adapter fills a slice of these by interpolating the sim's live mob instances; the
/// renderer groups them by species, frustum-culls, and bakes each with
/// [`mob_model::build_mob_instances`] against that species' model + texture.
#[derive(Clone, Debug, PartialEq)]
pub struct MobRenderInstance {
    /// Which species (selects the model / texture / draw buffers).
    pub kind: petramond::mob::Mob,
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
    pub blocklight: petramond_world::light::BlockLight6,
    /// Hurt-flash intensity in `[0, 1]`: tints the mob red after a non-lethal hit,
    /// fading out. `0` for an unhurt or dead mob.
    pub hurt: f32,
    /// Whether the mob is currently shorn: the bake skips the model's coat cubes
    /// (the ones named `wool`) so the fleece disappears until it regrows.
    pub shorn: bool,
    /// Multiply body tint from the mob's active named emitters (white when
    /// none) — e.g. the faint warm cast of a burning mob. Composed with the
    /// hurt flash and sampled light.
    pub emitter_tint: [f32; 3],
    /// Named model animations (mod-driven, replicated) as
    /// `(name, phase, weight)` — each is layered over the walk/idle/rest
    /// base pose at its OWN phase (seconds into the clip), scaled by its
    /// blend weight; names the model doesn't have are skipped.
    pub anims: Vec<(String, f32, f32)>,
    /// When the mob is dying, its per-bone ragdoll pose — `(rest-pivot position,
    /// rotation delta)` per bone in model space, already interpolated for this frame —
    /// used over the authored rest pose. `None` for a live mob. `Arc` so cloning a
    /// visible instance into its per-species batch stays cheap.
    pub ragdoll: Option<Arc<[(Vec3, Quat)]>>,
}

/// The local player's third-person body to draw this frame (absent in first
/// person): the compiled `player.bbmodel` at `pos` (feet), body facing
/// `body_yaw` with the head turned `head_yaw`/`head_pitch` relative to it,
/// walking (`moving`) at `anim_time` into the authored walk cycle. The held
/// item and its punch swing come from the renderer's own `HeldItemView` state —
/// the same animation the first-person hand plays.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlayerRenderInstance {
    /// World position of the feet (model `y=0`).
    pub pos: Vec3,
    /// Body facing yaw in radians (engine yaw space).
    pub body_yaw: f32,
    /// Head yaw relative to the body, and look pitch (radians).
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// Seconds into the walk animation.
    pub anim_time: f32,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle), eased by the
    /// game so starts/stops transition instead of snapping.
    pub walk_weight: f32,
    /// Sneak-stance blend weight (`0` upright … `1` crouched), eased like the
    /// walk blend. Cross-fades the authored `sneak` clip in: frame 0 while
    /// standing, its own cycle (instead of `walk`) while moving.
    pub sneak_weight: f32,
    /// Asleep in a bed: render lying on the back, feet at `pos`, head toward
    /// `body_yaw`; head-look and the arm swing are suppressed.
    pub sleeping: bool,
    /// Seated on a mob seat (mounted): thighs swing forward and shins hang
    /// from the knees, anchored at `pos` (the seat), walk/sneak layers rest;
    /// head-look and the arm swing stay live so a rider can look and punch.
    pub seated: bool,
    /// Hurt-flash intensity `[0, 1]` — tints the body red like a hurt mob.
    pub hurt: f32,
    /// 6-bit two-channel light sampled at the player.
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

/// One REMOTE player's body + held item to draw this frame, already
/// interpolated/posed by the game's presentation layer: the same
/// [`PlayerRenderInstance`] shape the local third-person body uses (so both
/// bake through `build_player_body` identically), plus that remote's OWN
/// [`HeldItemView`] animation channels — the local body instead reads the
/// renderer's internal first-person held-item view, keeping its solo
/// behavior bit-identical.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RemotePlayerRender {
    pub body: PlayerRenderInstance,
    pub held: HeldItemView,
    /// The remote's OFF-hand item view — drawn in the body's left hand
    /// (`item == None` = empty, nothing attached).
    pub held_off: HeldItemView,
}

/// A placed chest to draw in the world this frame: an inset body box plus a lid
/// hinged open by `lid01` (`0` closed .. `1` fully open), oriented to `facing` at the
/// block `pos` (the block's min corner). The game fills a slice of these from the
/// loaded chunks' chest block-entities; the renderer frustum-culls + bakes them with
/// [`chest_model::build_chests`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ChestInstance {
    /// World position of the block's min corner (block coords as f32).
    pos: Vec3,
    /// Placement orientation (which way the front + latch face).
    facing: petramond_math::facing::Facing,
    /// Lid open fraction: `0.0` closed, `1.0` fully open.
    lid01: f32,
    /// 6-bit skylight sampled from the world at the chest's cell.
    skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    blocklight: petramond_world::light::BlockLight6,
}

/// A placed door to draw in the world this frame: a 2-tall thin slab on the `facing`
/// edge of cell `pos` (the lower cell's min corner), swung open by `open01`
/// (`0` closed .. `1` fully open). The game fills a slice of these from the loaded
/// chunks' door state; the renderer frustum-culls + bakes them with
/// [`door_model::build_doors`]. The two halves carry different art (`bottom_tile` /
/// `top_tile`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DoorInstance {
    /// World position of the lower cell's min corner (block coords as f32).
    pos: Vec3,
    /// The edge the CLOSED door rests on (its outward normal); see [`petramond_world::door`].
    facing: petramond_math::facing::Facing,
    /// Swing fraction: `0.0` closed, `1.0` fully open onto the adjacent edge.
    open01: f32,
    /// Atlas tile for the lower half's front/back (door art).
    bottom_tile: petramond_world::tile::Tile,
    /// Atlas tile for the upper half's front/back (door art).
    top_tile: petramond_world::tile::Tile,
    /// Atlas tile for the four thin EDGE faces (the door's side — distinct from the
    /// front art, e.g. a plank strip).
    side_tile: petramond_world::tile::Tile,
    /// 6-bit skylight sampled from the world at the door's lower cell.
    skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    blocklight: petramond_world::light::BlockLight6,
}

/// A single terrain particle cube to draw this frame. `uv_min` / `uv_size` are
/// **absolute** atlas coordinates (sub-tile patch), produced by
/// `petramond::entity::Particle::atlas_uv`, so the particle pass samples the block
/// atlas directly with no further tile lookup.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ParticleInstance {
    pub pos: Vec3,
    /// Absolute atlas uv of the patch's min corner.
    pub uv_min: [f32; 2],
    /// Absolute atlas uv extent of the patch, per axis (the atlas is not square
    /// in normalized UV, so equal texel extents differ per axis).
    pub uv_size: [f32; 2],
    /// RGB tint multiplied into the sampled atlas colour (foliage-green for a
    /// grass/leaf fleck, white otherwise), from `petramond::entity::Particle::tint`.
    pub tint: [f32; 3],
    pub alpha: f32,
    /// World-space cube size (side length).
    pub size: f32,
    /// 6-bit skylight sampled from the world at the particle position.
    pub skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    pub blocklight: petramond_world::light::BlockLight6,
}

/// One SOLID-COLOR simulated particle this frame (an emitter-burst droplet —
/// water splash): already positioned by the particle system's physics, drawn
/// as an alpha-blended cube in the same pass as the looping-emitter cubes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SolidParticleInstance {
    pub pos: Vec3,
    pub color: [f32; 3],
    pub alpha: f32,
    pub size: f32,
    /// Vertical elongation of the cube around its centre (1 = a cube; rain
    /// streaks stretch tall).
    pub stretch: f32,
    /// 6-bit light sampled at the particle, folded into the color.
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

/// One VISIBLE particle emitter to draw this frame (a block row's or a mob's).
/// The renderer turns this declarative row into transient translucent cube
/// particles; no state is persisted.
///
/// The same row the gather produces — the frame carries one emitter type end to
/// end, so nothing between the world and the vertex builder is a re-map.
pub use petramond::world::PlacedEmitter as ParticleEmitterInstance;
