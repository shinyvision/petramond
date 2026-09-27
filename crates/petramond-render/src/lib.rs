//! WGPU renderer: atlas texture, opaque + transparent pipelines, fog.

pub mod atlas;
pub mod block_draw;
pub mod block_entity_model;
pub mod break_overlay;
pub mod camera;
mod content_limits;
pub mod crosshair;
pub mod effect_icons;
pub mod entity_shadow;
pub(crate) mod first_person;
pub mod foliage_tint;
pub mod geometry_arena;
pub mod gpu_mem;
pub mod gpu_timer;
pub mod hand;
mod held_view;
pub mod item_cube;
pub mod item_entity;
pub mod item_model;
pub mod job;
pub mod views;
pub mod world_marks;

pub mod lighting;
pub mod mob_model;
mod model_break;
pub mod particles;
pub mod pipeline;
pub mod player_model;
pub mod renderer;
pub mod resources;
pub mod scene;
pub mod selection;
mod selection_highlight;
pub mod shader_pack;
pub(crate) mod skinned;
pub mod texture_mips;
pub mod ui;
pub mod uniforms;

pub use held_view::HeldItemEase;
pub use renderer::new_offscreen_renderer;
pub use renderer::new_renderer_from_target;
#[allow(unused_imports)]
pub use renderer::TerrainMemory;
pub use renderer::{
    CaptureRequest, CaptureSource, Captured, GhostPiece, RenderFailure, RenderInitError,
    RenderedFrame, Renderer, SchematicThumbnailer,
};
pub use views::BreakOverlayView;
pub use views::EntityShadow;
pub use views::{AnimId, AnimInterner, AnimLayer, AnimNames, GaitFade, MobArena};
pub use world_marks::{WorldMark, WorldMarks};

pub use scene::Scene;

use glam::Vec3;
use petramond_math::math::Tilt;
use petramond_world::block_state::HeldBlockState;
use petramond_world::item::ItemType;
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

/// One entry of the client-overlay layer, drawn in order.
pub enum ClientOverlayItem {
    Image(ClientOverlayImage),
    /// Batches `[start, end)` of [`ClientOverlayLayer::paint`]: solid and
    /// glyph quads in physical px, each batch scissored to its own clip.
    Paint {
        batches: std::ops::Range<usize>,
    },
}

/// The physical-pixel client layer — overlays and a modal canvas — as one
/// ordered list, so a canvas's rules, labels and images stack in the order
/// its mod retained them.
#[derive(Default)]
pub struct ClientOverlayLayer {
    pub items: Vec<ClientOverlayItem>,
    pub paint: petramond_ui::DrawList,
}

impl ClientOverlayLayer {
    pub fn clear(&mut self) {
        self.items.clear();
        self.paint.clear();
    }

    pub fn push_image(&mut self, image: ClientOverlayImage) {
        self.items.push(ClientOverlayItem::Image(image));
    }

    /// Paint one run of quads into the layer, in order after everything
    /// already in it.
    pub fn paint(&mut self, draw: impl FnOnce(&mut petramond_ui::DrawList)) {
        // Sealing keeps this run's first quad from merging into the previous
        // run's last batch, which would draw it under an image pushed between.
        self.paint.begin_overlay();
        let start = self.paint.batches.len();
        draw(&mut self.paint);
        let end = self.paint.batches.len();
        if end > start {
            self.items.push(ClientOverlayItem::Paint {
                batches: start..end,
            });
        }
    }
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

/// Both UI layers of one render frame. The SCENE layer draws over the world
/// into the frame and is what a capture of the frame records with it; the
/// WINDOW layer is composited onto the window only, after the frame's capture
/// point. The renderer accepts or rejects the pair as a whole.
pub struct UiLayers<'a> {
    pub scene: UiFrame<'a>,
    pub window: UiFrame<'a>,
}

/// One UI layer's handoff for one render frame. Every physical-pixel part
/// consumes `viewport`.
pub struct UiFrame<'a> {
    pub viewport: petramond::gui::UiViewport,
    pub document: Option<DocumentUiFrame<'a>>,
    pub content: &'a petramond::gui::UiSnapshot,
    pub client_overlays: &'a ClientOverlayLayer,
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
        let overlays = ClientOverlayLayer::default();
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
            client_overlays: &overlays,
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
            client_overlays: &overlays,
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

/// One hand's held item as the seats and attaches read it: the item whose art
/// draws, the held stack's authored hold, and the claimed held pose, eased
/// ([`HeldItemEase`]).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldItemView {
    /// The item whose ART draws: the held stack, or a mod's display stand-in
    /// for it (`HeldItemFrame::display`).
    pub item: Option<ItemType>,
    /// The authored hold of the REAL held stack — always the stack's, never
    /// the display item's, so a stand-in's art sits exactly where the hand
    /// holds what it actually holds.
    pub hold: petramond_world::item::HeldPose,
    /// The held stack's instance-data variant (tint resolution at draw).
    pub variant: petramond_world::item::VariantId,
    pub block_state: HeldBlockState,
    /// The hand's claimed held pose, already EASED so a 20 Hz publisher
    /// still glides. Identity on ordinary frames.
    pub pose: HeldPose,
}

/// A claimed held-item pose: one extra Blockbench display transform per VIEW,
/// composed onto whatever hold the item already has.
///
/// It is a [`DisplayTransform`] because that is exactly what it is, so the
/// engine's existing composition ([`DisplayTransform::base_matrix`]) and
/// left-hand mirroring ([`DisplayTransform::left_hand`], reached here by
/// conjugating the hand frame) apply unchanged. Two views because they start
/// from different authored holds, so one intent is a different delta in each.
///
/// [`DisplayTransform`]: petramond_world::block_model::DisplayTransform
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct HeldPose {
    pub first_person: petramond_world::block_model::DisplayTransform,
    pub third_person: petramond_world::block_model::DisplayTransform,
}

/// How fast a published presentation POSE chases its target, per second
/// (first-order lag, like the hand's bob).
///
/// Fast enough that a deliberate raise reads as instant, slow enough to turn a
/// replicated publisher's 20 Hz steps into a glide.
///
/// ONE rate for everything posed — the item in a hand ([`HeldPose`]) and the
/// claimed bone offsets carrying it alike. A body whose arm snaps to a stance
/// while the thing in its fist glides there is the item visibly trailing its
/// own hand.
pub const POSE_EASE_RATE: f32 = 22.0;

impl HeldPose {
    /// Ease each view a fraction `t` toward `to`, on the two channels a claim
    /// may set. Rotation eases in DEGREES rather than as a quaternion: these
    /// are small deltas from an authored hold, where the two agree, and staying
    /// a `DisplayTransform` keeps every consumer on one type.
    pub fn ease_toward(&mut self, to: &HeldPose, t: f32) {
        let ease = |a: &mut petramond_world::block_model::DisplayTransform,
                    b: &petramond_world::block_model::DisplayTransform| {
            for i in 0..3 {
                a.rotation[i] += (b.rotation[i] - a.rotation[i]) * t;
                a.translation[i] += (b.translation[i] - a.translation[i]) * t;
            }
        };
        ease(&mut self.first_person, &to.first_person);
        ease(&mut self.third_person, &to.third_person);
    }
}

impl Default for HeldItemView {
    fn default() -> Self {
        HeldItemView {
            item: None,
            hold: petramond_world::item::HeldPose::DEFAULT,
            variant: petramond_world::item::VariantId::NONE,
            block_state: HeldBlockState::None,
            pose: HeldPose::default(),
        }
    }
}

/// Sim intent for one hand this frame: what it holds and its levels — the
/// client animator drivers' per-hand inputs, and what [`HeldItemEase`] turns
/// into the hand's [`HeldItemView`]. The hand's one-shot gestures are not
/// here: they reach the drivers as resolved graph events.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct HeldItemFrame {
    pub item: Option<ItemType>,
    /// The item whose ART draws in place of `item`'s (a mod's held-display
    /// claim — a launcher through its draw frames); `None` = `item`'s own.
    /// Only the look: the hand's authored hold and every reset keyed on the
    /// held item come from `item`, so a display changing under a hand never
    /// restarts its eased pose, and a display-only row needs no hold of its
    /// own.
    pub display: Option<ItemType>,
    /// The held stack's instance-data variant (tint resolution at draw).
    pub variant: petramond_world::item::VariantId,
    pub block_state: HeldBlockState,
    pub mining: bool,
    /// Level: a food item is mid-eat, carrying the eat's progress in `[0, 1)`.
    /// The animator raises the food quickly at the start, then drifts it the
    /// rest of the way to the mouth as the progress advances.
    pub eating: Option<f32>,
    /// This hand's claimed held pose as last published — predicted locally this
    /// frame, or replicated from the authority. `None` = the item's authored
    /// hold. The animator eases toward it; geometry reads the eased
    /// [`HeldItemView::pose`].
    pub pose_target: Option<HeldPose>,
}

/// The local player's presentation for one frame, built by the client's
/// animation stage: both hands' eased held views and the first-person
/// viewmodel's posed bones. [`Renderer::update_uniforms`] draws the world
/// through the viewmodel's camera bone, so the client hands this over first.
#[derive(Clone, Copy, Debug)]
pub struct LocalFrame<'a> {
    /// Each hand's eased held view, `[main, off]` (`item == None` draws
    /// nothing in that hand).
    pub held: [HeldItemView; 2],
    /// The viewmodel rig's posed bones (model space, rig pixels); empty
    /// without a viewmodel animator, which leaves the rig at rest.
    pub first_person: &'a [glam::Mat4],
}

/// How a dropped item entity is turned this frame. The contract holds for
/// every render kind — cube, extruded sprite and bbmodel alike answer a pose
/// the same way (`item_entity::Placement`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ItemEntityPose {
    /// A loose stack: spun about Y by this many radians, hovering and
    /// bobbing above its `pos`, drawn as a layered pile per `count`.
    Spin(f32),
    /// An item in flight or lodged in a block: yawed about Y and PITCHED up
    /// from level about the entity's own centre (no hover, no bob), still,
    /// as one piece whatever the count; `speed` (m/s, 0 once lodged) draws a
    /// fast one trailing its path.
    Aimed { yaw: f32, pitch: f32, speed: f32 },
}

/// A dropped item-entity to draw in the world this frame: a small spinning +
/// bobbing cube (or extruded 3D slab for sprite-kind items) at `pos`, turned
/// per `pose`. The App fills a slice of these from its `DroppedItem`s.
/// A stack draws as several offset, layered copies (capped at 5) per `count`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemEntityInstance {
    pub pos: petramond_math::world_pos::WorldPos,
    pub item: ItemType,
    /// The stack's instance-data variant (tint resolution at draw).
    pub variant: petramond_world::item::VariantId,
    /// Stack size. Drives how many layered geometries the pile draws (1..=5).
    pub count: u8,
    pub pose: ItemEntityPose,
    /// 6-bit skylight sampled from the world at the dropped item's position.
    pub skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    pub blocklight: petramond_world::light::BlockLight6,
}

/// One placed block's mod DRAW SET to draw this frame. Owned by the gather
/// that produces it (`world::draw`) and carried here UNCHANGED — see its doc.
pub use petramond::world::draw::BlockDrawInstance;

/// A mob's base animation: the clip its locomotion state selects, under
/// every named layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GaitClip {
    Walk,
    Idle(u8),
}

/// One animated mob to draw in the world this frame: a species (`kind`) posed at
/// `anim_time` into its walk cycle (when `moving`; otherwise its rest pose), placed
/// at `pos` (its feet) facing `yaw`, lit by the sampled `skylight`. The scene
/// adapter fills a slice of these by interpolating the sim's live mob instances; the
/// renderer groups them by species, frustum-culls, and poses each with
/// `mob_model::pose_mob_instances` for that species' skinned mesh + texture.
///
/// A plain `Copy` row: its variable-length parts (fading gaits, named layers,
/// ragdoll bones) are ranges into the frame's [`MobArena`], like a body's
/// [`BoneRange`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobRenderInstance {
    /// Which species (selects the model / texture / draw buffers).
    pub kind: petramond::mob::Mob,
    /// World position of the mob's feet (model `y=0`).
    pub pos: petramond_math::world_pos::WorldPos,
    /// Facing yaw in radians (rotation about Y).
    pub yaw: f32,
    /// Body tilt applied inside the yaw; level for every body the engine
    /// moves itself.
    pub tilt: Tilt,
    /// Seconds into the active animation (walk or idle_*); used unless idle+resting.
    pub anim_time: f32,
    /// Whether the mob is walking this frame: plays the walk animation if so.
    pub moving: bool,
    /// When idle, which `idle_*` animation is playing (index), or `None` for the
    /// neutral rest pose.
    pub idle_anim: Option<u8>,
    /// How far the active gait (walk or idle) has eased in, 0..1.
    pub gait_weight: f32,
    /// Gaits eased out of and still fading, as a range into
    /// [`MobArena::gait_fades`].
    pub gait_fades: ArenaRange,
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
    /// Multiply body tint from the mob's active emitter bundles (white when
    /// none) — e.g. the faint warm cast of a burning mob. Composed with the
    /// hurt flash and sampled light.
    pub emitter_tint: [f32; 3],
    /// How much of its light the body provides itself (`0..=1`, the strongest
    /// active emitter's `body_self_lit`): a burning body stays visible in the dark.
    pub emitter_self_lit: f32,
    /// Named model animations (mod-driven, replicated) as a range into
    /// [`MobArena::anims`] — each is layered over the walk/idle/rest base pose
    /// at its OWN phase (seconds into the clip), scaled by its blend weight;
    /// names the model doesn't have are skipped.
    pub anims: ArenaRange,
    /// When the mob is dying, its per-bone ragdoll pose — `(rest-pivot position,
    /// rotation delta)` per bone in model space, already interpolated for this frame —
    /// used over the authored rest pose, as a range into [`MobArena::ragdoll`].
    /// `None` for a live mob.
    pub ragdoll: Option<ArenaRange>,
    /// The items drawn in the species' main and off hand bones.
    pub held: [Option<petramond_world::item::ItemType>; 2],
}

/// One row's slice of a frame arena (a body's posed bones).
///
/// Rows carry a RANGE rather than their own list so a render instance stays
/// a plain `Copy` value with no per-body allocation, and so nothing has to cap
/// how many entries a body may wear.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ArenaRange {
    pub start: u32,
    pub len: u32,
}

impl ArenaRange {
    /// The next `len` rows of `arena` (the ones a caller is about to append).
    pub fn next(arena: &[impl Sized], len: usize) -> Self {
        Self {
            start: arena.len() as u32,
            len: len as u32,
        }
    }

    /// The rows appended to `arena` since it held `start` of them.
    pub fn since(arena: &[impl Sized], start: usize) -> Self {
        Self {
            start: start as u32,
            len: arena.len().saturating_sub(start) as u32,
        }
    }

    /// This range's rows. An out-of-bounds range (a stale row, never a
    /// correctly built one) reads as empty rather than panicking mid-frame.
    pub fn of<T>(self, arena: &[T]) -> &[T] {
        let start = self.start as usize;
        arena
            .get(start..start + self.len as usize)
            .unwrap_or(&[][..])
    }
}

/// One posed player body to draw this frame: the body rig skinned from the
/// bones the client's animation posed, placed at `pos` (feet). The renderer
/// computes no pose — it only places, lights and tints what it is given.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlayerRenderInstance {
    /// Multiply body tint from the body's active emitter bundles (its
    /// conditions' stage emitters), composed with the hurt flash.
    pub emitter_tint: [f32; 3],
    /// How much of its light the body provides itself (`0..=1`, the strongest
    /// active emitter's `body_self_lit`): a burning body stays visible in the dark.
    pub emitter_self_lit: f32,
    /// World position of the feet (model `y=0`).
    pub pos: petramond_math::world_pos::WorldPos,
    /// Asleep in a bed: the hands stay empty (the held items would poke
    /// through the bed).
    pub sleeping: bool,
    /// Hurt-flash intensity `[0, 1]` — tints the body red like a hurt mob.
    pub hurt: f32,
    /// 6-bit two-channel light sampled at the player.
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
    /// This body's posed bones — model-space matrices in rig pixels, one per
    /// bone of the body rig — as a range into the frame's pose arena.
    pub pose: ArenaRange,
    /// Stands the posed rig about `pos`: body yaw, seat lean or the lying
    /// turn, and the model scale. The renderer prepends only the translation
    /// to the feet.
    pub placement: glam::Mat4,
}

/// One player body to draw this frame — the local third-person body or a
/// remote — posed by the client's animation, with the eased [`HeldItemView`]
/// of each hand.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlayerBodyRender {
    pub body: PlayerRenderInstance,
    pub held: HeldItemView,
    /// The OFF-hand item view — drawn in the body's left hand (`item ==
    /// None` = empty, nothing attached).
    pub held_off: HeldItemView,
}

/// A placed ANIMATED block to draw in the world this frame — a chest, a door, a
/// trapdoor, any row with an animated model: its block (which names the model
/// and supplies the row tiles), the variant and facing its cell's state poses
/// it in, and how far open it has eased (`0` closed .. `1` fully open). The
/// game fills a slice of these from the loaded chunks' animated blocks; the
/// renderer frustum-culls + bakes them all with
/// [`block_entity_model::push_block_entities`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlockEntityInstance {
    /// The block's cell (a compound's anchor — a door's lower half).
    pos: petramond_math::math::IVec3,
    /// The row drawn; its animated model is the geometry.
    block: petramond_world::block::Block,
    /// The direction the model's canonical `+Z` front is turned to.
    facing: petramond_math::facing::Facing,
    /// Which of the model's variants is drawn.
    variant: u8,
    /// Eased open fraction: `0.0` closed, `1.0` fully open.
    open01: f32,
    /// 6-bit skylight sampled from the world at the cell.
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
    /// World-space half-axes for a double-sided textured quad, or None for a cube.
    pub quad_axes: Option<[Vec3; 2]>,
    pub pos: petramond_math::world_pos::WorldPos,
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
    pub pos: petramond_math::world_pos::WorldPos,
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

mod schematic;
