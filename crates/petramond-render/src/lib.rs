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

#[derive(Clone)]
pub struct ClientOverlayImage {
    pub key: String,
    pub size: (u16, u16),
    pub rgba: Arc<[u8]>,
    pub revision: u64,
    pub recent_blits: Vec<(u64, [u16; 4])>,
    pub rect: [f32; 4],
    pub uv: [f32; 4],
}

pub enum ClientOverlayItem {
    Image(ClientOverlayImage),
    Paint { batches: std::ops::Range<usize> },
}

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

    pub fn paint(&mut self, draw: impl FnOnce(&mut petramond_ui::DrawList)) {
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

pub struct DocumentUiFrame<'a> {
    pub viewport: petramond::gui::UiViewport,
    pub kind: petramond_world::gui_state::GuiKind,
    pub draw: &'a petramond_ui::DrawList,
    pub images: &'a [petramond::gui::DocImageSource],
    pub slots: &'a [petramond::gui::DocSlot],
    pub hooks: &'a [petramond::gui::DocHook],
}

pub struct UiLayers<'a> {
    pub scene: UiFrame<'a>,
    pub window: UiFrame<'a>,
}

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

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldItemView {
    pub item: Option<ItemType>,
    pub hold: petramond_world::item::HeldPose,
    pub variant: petramond_world::item::VariantId,
    pub block_state: HeldBlockState,
    pub pose: HeldPose,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct HeldPose {
    pub first_person: petramond_world::block_model::DisplayTransform,
    pub third_person: petramond_world::block_model::DisplayTransform,
}

pub const POSE_EASE_RATE: f32 = 22.0;

impl HeldPose {
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

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct HeldItemFrame {
    pub item: Option<ItemType>,
    pub display: Option<ItemType>,
    pub variant: petramond_world::item::VariantId,
    pub block_state: HeldBlockState,
    pub mining: bool,
    pub eating: Option<f32>,
    pub pose_target: Option<HeldPose>,
}

#[derive(Clone, Copy, Debug)]
pub struct LocalFrame<'a> {
    pub held: [HeldItemView; 2],
    pub first_person: &'a [glam::Mat4],
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ItemEntityPose {
    Spin(f32),
    Aimed {
        yaw: f32,
        pitch: f32,
        speed: f32,
        spin: f32,
    },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemEntityInstance {
    pub pos: petramond_math::world_pos::WorldPos,
    pub item: ItemType,
    pub variant: petramond_world::item::VariantId,
    pub count: u8,
    pub pose: ItemEntityPose,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

pub use petramond::world::draw::BlockDrawInstance;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GaitClip {
    Walk,
    Idle(u8),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobRenderInstance {
    pub kind: petramond::mob::Mob,
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    pub tilt: Tilt,
    pub anim_time: f32,
    pub moving: bool,
    pub idle_anim: Option<u8>,
    pub gait_weight: f32,
    pub gait_fades: ArenaRange,
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
    pub hurt: f32,
    pub shorn: bool,
    pub emitter_tint: [f32; 3],
    pub emitter_self_lit: f32,
    pub anims: ArenaRange,
    pub ragdoll: Option<ArenaRange>,
    pub held: [Option<petramond_world::item::ItemType>; 2],
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ArenaRange {
    pub start: u32,
    pub len: u32,
}

impl ArenaRange {
    pub fn next(arena: &[impl Sized], len: usize) -> Self {
        Self {
            start: arena.len() as u32,
            len: len as u32,
        }
    }

    pub fn since(arena: &[impl Sized], start: usize) -> Self {
        Self {
            start: start as u32,
            len: arena.len().saturating_sub(start) as u32,
        }
    }

    pub fn of<T>(self, arena: &[T]) -> &[T] {
        let start = self.start as usize;
        arena
            .get(start..start + self.len as usize)
            .unwrap_or(&[][..])
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlayerRenderInstance {
    pub emitter_tint: [f32; 3],
    pub emitter_self_lit: f32,
    pub pos: petramond_math::world_pos::WorldPos,
    pub sleeping: bool,
    pub hurt: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
    pub pose: ArenaRange,
    pub placement: glam::Mat4,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlayerBodyRender {
    pub body: PlayerRenderInstance,
    pub held: HeldItemView,
    pub held_off: HeldItemView,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlockEntityInstance {
    pos: petramond_math::math::IVec3,
    block: petramond_world::block::Block,
    facing: petramond_math::facing::Facing,
    variant: u8,
    open01: f32,
    skylight: u8,
    blocklight: petramond_world::light::BlockLight6,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ParticleInstance {
    pub quad_axes: Option<[Vec3; 2]>,
    pub pos: petramond_math::world_pos::WorldPos,
    pub uv_min: [f32; 2],
    pub uv_size: [f32; 2],
    pub tint: [f32; 3],
    pub alpha: f32,
    pub size: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SolidParticleInstance {
    pub pos: petramond_math::world_pos::WorldPos,
    pub color: [f32; 3],
    pub alpha: f32,
    pub size: f32,
    pub stretch: f32,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

pub use petramond::world::PlacedEmitter as ParticleEmitterInstance;

mod schematic;
