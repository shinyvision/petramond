use super::gpu_timer;
use crate::camera::{Camera, Containment, Frustum, ViewVolume};
use petramond::world::TerrainRenderHandoff;
use petramond_world::chunk::ChunkPos;
use petramond_world::selection::SelectionShape;

use std::collections::HashMap;
use wgpu::util::DeviceExt;

mod actor_pass;
use actor_pass::{ActorPass, MobGpu, PlayerGpu};
use skinned_draw::{SkinFrame, SkinnedModel};
mod capture;
mod client_overlay;
mod column_store;
use column_store::{ColumnSlot, ColumnStore};
mod construct;
mod error;
use error::DeviceHealth;
pub use error::{RenderFailure, RenderInitError};
mod ghosts;
mod graph;
pub use ghosts::GhostPiece;
use graph::{FrameGraph, FramePlan};
mod schematic_thumbnail;
pub use schematic_thumbnail::SchematicThumbnailer;
mod doc_ui;
mod draw_plan;
mod dynamic_bake;
pub(crate) mod dynamic_draw;
mod frame;
mod frame_state;
mod hand_pass;
mod selection;
mod skinned_draw;
use hand_pass::HandPass;
mod hand_bake;
mod icon_atlas;
mod lod;
mod offscreen;
mod passes;
use passes::Node;
mod post_process;
mod readback;
mod sized_frames;
mod ui_frame;
mod upload_queue;
use upload_queue::UploadQueue;
mod world_marks;

pub use capture::{CaptureRequest, CaptureSource, Captured};
#[cfg(test)]
pub(crate) use construct::instance_descriptor;
pub use construct::new_renderer_from_target;
use dynamic_draw::{DynamicDraw, DynamicInstanceDraw, DynamicVertexDraw};
use icon_atlas::IconAtlas;
use lod::far_leaf_lod_active;
pub use offscreen::new_offscreen_renderer;
pub use offscreen::RenderedFrame;

use super::block_entity_model::push_block_entities;
use super::break_overlay::build_break_overlays;
use super::crosshair::crosshair_vertices;
use super::entity_shadow::{build_entity_shadows, ShadowVertex};
use super::item_entity::build_item_entities;
use super::item_model::ItemVertex;
use super::mob_model::pose_mob_instances;
use super::particles::{build_particles_split, build_transparent_emitter_particles};
use super::pipeline::{create_pipeline_resources, EnvPassResources};
use super::resources::{
    create_atlas, create_atlas_array, create_gui_panel, create_model_texture, create_scene_color,
    upload_column_mesh, ColumnOrigins, GpuSectionMesh, SectionStream, Span,
};
use super::selection::outline_vertices;
use super::ui::{build_ui, UiBuild, UiVertex};
use super::uniforms::Uniforms;
use super::{
    BlockEntityInstance, BreakOverlayView, EntityShadow, HeldItemView, ItemEntityInstance,
    LocalFrame, MobRenderInstance, ParticleEmitterInstance, ParticleInstance, PlayerBodyRender,
    SolidParticleInstance, UiFrame, UiLayers,
};
use draw_plan::{QuadPass, SectionOcclusion, TerrainDraws};
use petramond::gui::{UiSnapshot, UiViewport};
use petramond_world::bbmodel::Model;

const TERRAIN_FOG_CULL_PAD: f32 = 32.0;

#[derive(Copy, Clone, PartialEq, Eq)]
struct TerrainViewKey {
    view_proj: [u32; 16],
    cam: [u64; 3],
    fog: u32,
}

pub use crate::camera::aabb_distance_sq;

#[derive(Copy, Clone, Debug)]
pub struct TerrainMemory {
    pub arena_bytes: u64,
    pub arena_blocks: usize,
    pub arena_free: u64,
    pub suballocated: u64,
    pub used: u64,
    pub live_allocs: usize,
    pub suballocs_since_start: u64,
}

#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct RenderStats {
    pub opaque_draws: u32,
    pub transparent_draws: u32,
    pub opaque_indices: u64,
    pub transparent_indices: u64,
}

#[derive(Copy, Clone)]
pub(crate) struct VisibleSection {
    dist_sq: f32,
    column_pos: ChunkPos,
    column_slot: ColumnSlot,
    opaque_batched: bool,
    model_batched: bool,
    use_far_leaf_lod: bool,
    spans: [Span; SectionStream::COUNT],
}

impl VisibleSection {
    #[inline]
    fn span(&self, stream: SectionStream) -> Span {
        self.spans[stream.index()]
    }
}

struct EnvPass {
    res: EnvPassResources,
    bind: wgpu::BindGroup,
    dormant: bool,
}

struct ParticlePass {
    draw: DynamicInstanceDraw<super::particles::ParticleRow>,
    emitter_draw: DynamicInstanceDraw<super::particles::ParticleRow>,
    instances: Vec<ParticleInstance>,
    model_instances: Vec<ParticleInstance>,
    solid_instances: Vec<SolidParticleInstance>,
    emitters: Vec<ParticleEmitterInstance>,
    density: f32,
    block_count: u32,
    rows: Vec<super::particles::ParticleRow>,
    emitter_rows: Vec<super::particles::ParticleRow>,
    emitter_scratch: Vec<super::particles::TransparentParticleCube>,
}

impl ParticlePass {
    fn clear_world(&mut self) {
        self.draw.instance_count = 0;
        self.emitter_draw.instance_count = 0;
        self.block_count = 0;
        self.instances.clear();
        self.model_instances.clear();
        self.solid_instances.clear();
        self.emitters.clear();
    }
}

struct ItemEntityPass {
    draw: DynamicDraw,
    verts: Vec<petramond_mesh::Vertex>,
    indices: Vec<u32>,
    visible: Vec<ItemEntityInstance>,
    instances: Vec<ItemEntityInstance>,
    block_draws: Vec<crate::BlockDrawInstance>,
    block_draws_visible: Vec<u32>,
    model_draw: DynamicDraw,
    model_verts: Vec<super::item_model::ItemVertex>,
    model_indices: Vec<u32>,
    sprite_draw: DynamicDraw,
    sprite_verts: Vec<super::item_model::ItemVertex>,
    sprite_indices: Vec<u32>,
    sprite_scratch: Vec<super::item_model::ItemVertex>,
}

impl ItemEntityPass {
    fn clear_world(&mut self) {
        self.draw.index_count = 0;
        self.model_draw.index_count = 0;
        self.sprite_draw.index_count = 0;
        self.instances.clear();
        self.visible.clear();
    }
}

struct ShadowPass {
    draw: DynamicVertexDraw,
    verts: Vec<ShadowVertex>,
    instances: Vec<EntityShadow>,
}

impl ShadowPass {
    fn clear_world(&mut self) {
        self.draw.vertex_count = 0;
        self.instances.clear();
    }
}

struct BlockEntityPass {
    draw: DynamicDraw,
    instances: Vec<BlockEntityInstance>,
    visible: Vec<BlockEntityInstance>,
    baked: Vec<BlockEntityInstance>,
    baked_origin: glam::IVec3,
}

impl BlockEntityPass {
    fn clear_world(&mut self) {
        self.draw.index_count = 0;
        self.instances.clear();
        self.visible.clear();
        self.baked.clear();
        self.baked_origin = glam::IVec3::MIN;
    }
}

struct TerrainPipes {
    opaque: crate::pipeline::SampledPipeline,
    translucent: crate::pipeline::SampledPipeline,
    transparent: crate::pipeline::SampledPipeline,
    transparent_two_sided: crate::pipeline::SampledPipeline,
    world_model: crate::pipeline::SampledPipeline,
    world_model_blend: crate::pipeline::SampledPipeline,
    contact: crate::pipeline::SampledPipeline,
}

#[derive(Default)]
struct TerrainPlan {
    sections: Vec<VisibleSection>,
    opaque_columns: Vec<OpaqueColumnDraw>,
    model_columns: Vec<(f32, ChunkPos, ColumnSlot)>,
    contact_columns: Vec<(f32, ChunkPos, ColumnSlot)>,
    any_model: bool,
    any_transparent: bool,
}

impl TerrainPlan {
    fn clear(&mut self) {
        self.sections.clear();
        self.opaque_columns.clear();
        self.model_columns.clear();
        self.contact_columns.clear();
        self.any_model = false;
        self.any_transparent = false;
    }
}

struct TerrainPass {
    pipes: TerrainPipes,
    columns: ColumnStore,
    column_origins: ColumnOrigins,
    geometry: super::resources::TerrainArenas,
    /// Shared index buffer for the implied-triangulation terrain streams.
    quad_index: super::resources::QuadIndexBuffer,
    uploads: UploadQueue,
    plan: TerrainPlan,
    draws: TerrainDraws,
    sort_scratch: Vec<(f32, ChunkPos, u32)>,
    sorted_scratch: Vec<VisibleSection>,
    gpu_revision: u64,
    planned_gpu_revision: u64,
    view_key: TerrainViewKey,
    planned_view_key: Option<TerrainViewKey>,
    /// Dense mirror of the column set for the per-frame cull: just the AABB
    /// inputs, in one contiguous array. The planner rejects the great majority
    /// of columns and the rejection must not walk a hash map of
    /// [`GpuColumnMesh`]es — each is a large record owning a separately
    /// allocated section list. Rebuilt only when the column set changes.
    cull_index: Vec<ColumnCull>,
    cull_regions: Vec<CullRegion>,
    cull_index_revision: u64,
    occlusion: SectionOcclusion,
}

const CULL_REGION_SHIFT: i32 = 3;
const CULL_REGION_COLUMNS: i32 = 1 << CULL_REGION_SHIFT;

pub(crate) type OpaqueColumnDraw = (f32, ChunkPos, ColumnSlot, bool);

#[derive(Copy, Clone)]
struct CullRegion {
    cx: i32,
    cz: i32,
    min_cy: i32,
    max_cy: i32,
    first: u32,
    last: u32,
}

#[derive(Copy, Clone)]
struct ColumnCull {
    pos: ChunkPos,
    slot: ColumnSlot,
    min_cy: i32,
    max_cy: i32,
}

impl TerrainPass {
    fn clear_world(&mut self) {
        self.columns.clear();
        self.uploads.clear();
        self.gpu_revision = self.gpu_revision.wrapping_add(1);
        self.planned_view_key = None;
        self.cull_index.clear();
        self.cull_regions.clear();
        self.occlusion.clear();
        self.cull_index_revision = u64::MAX;
        self.plan.clear();
        self.draws.clear();
    }
}

struct UiPass {
    pipe: wgpu::RenderPipeline,
    texture_bgl: wgpu::BindGroupLayout,
    theme: Option<doc_ui::ThemeBinds>,
    icon_atlas: IconAtlas,
    scene: UiLayer,
    window: UiLayer,
    scene_generation: u64,
    window_generation: u64,
}

struct UiLayer {
    doc_ui: doc_ui::DocUi,
    client_overlays: client_overlay::ClientOverlays,
    solid_vbuf: wgpu::Buffer,
    solid_verts: Vec<UiVertex>,
    count_vertex_count: u32,
    overlay_count_vertex_count: u32,
    drag_count_vertex_count: u32,
    hud_layers: Vec<HudLayer>,
    icon_quad_vbuf: wgpu::Buffer,
    icon_quad_verts: Vec<UiVertex>,
    icon_quad_vertex_count: u32,
    overlay_icon_quad_vertex_count: u32,
    drag_icon_quad_vertex_count: u32,
    build: UiBuild,
    prepared_viewport: UiViewport,
}

impl UiLayer {
    fn new(device: &wgpu::Device, hud_layers: Vec<HudLayer>) -> Self {
        let buffer = |label| dynamic_draw::new_buffer(device, wgpu::BufferUsages::VERTEX, label);
        Self {
            doc_ui: doc_ui::DocUi::default(),
            client_overlays: client_overlay::ClientOverlays::default(),
            solid_vbuf: buffer("ui solid vbuf"),
            solid_verts: Vec::new(),
            count_vertex_count: 0,
            overlay_count_vertex_count: 0,
            drag_count_vertex_count: 0,
            hud_layers,
            icon_quad_vbuf: buffer("icon quad vbuf"),
            icon_quad_verts: Vec::new(),
            icon_quad_vertex_count: 0,
            overlay_icon_quad_vertex_count: 0,
            drag_icon_quad_vertex_count: 0,
            build: UiBuild::default(),
            prepared_viewport: UiViewport::default(),
        }
    }
}

struct SkyPass {
    pipe: crate::pipeline::SampledPipeline,
    bind: wgpu::BindGroup,
    texture_bind: wgpu::BindGroup,
    shader_param_keys: Vec<String>,
    light_param_key: Option<String>,
    env_passes: Vec<EnvPass>,
    env_scaler: super::pipeline::EnvScalers,
    env_color: wgpu::TextureView,
    env_depth: wgpu::TextureView,
    env_down_bind: wgpu::BindGroup,
    env_comp_bind: wgpu::BindGroup,
    fog_start: f32,
    fog_end: f32,
    scale: f32,
    color: [f32; 3],
    clear_color: [f32; 3],
}

struct ChromePass {
    outline_pipe: crate::pipeline::SampledPipeline,
    outline_bind: wgpu::BindGroup,
    outline_vbuf: wgpu::Buffer,
    outline_vertex_count: u32,
    crosshair_pipe: wgpu::RenderPipeline,
    crosshair_vbuf: wgpu::Buffer,
    crosshair_vertex_count: u32,
    crosshair_drawn_size: (u32, u32),
    crosshair_visible: bool,
    selection: Option<SelectionShape>,
    selection_drawn: Option<(SelectionShape, glam::IVec3)>,
}

impl ChromePass {
    fn clear_world(&mut self) {
        self.selection = None;
        self.selection_drawn = None;
        self.outline_vertex_count = 0;
        self.crosshair_visible = false;
        self.crosshair_vertex_count = 0;
    }
}

struct SceneTargets {
    scene_color: wgpu::TextureView,
    multisample_color: Option<wgpu::TextureView>,
    max_samples: u32,
    depth: wgpu::TextureView,
    render_scale: f32,
    grade_enabled: bool,
    anti_aliasing: petramond::save::client::AntiAliasing,
    grade_pipe: wgpu::RenderPipeline,
    grade_bgl: wgpu::BindGroupLayout,
    grade_bind: wgpu::BindGroup,
    post_process_buf: wgpu::Buffer,
    mood: [f32; 2],
}

struct SharedBinds {
    uniform_buf: wgpu::Buffer,
    shader_params_buf: wgpu::Buffer,
    uniform: wgpu::BindGroup,
    atlas: wgpu::BindGroup,
    atlas_array: wgpu::BindGroup,
    model_atlas: wgpu::BindGroup,
}

struct ViewState {
    frustum: Frustum,
    cam_pos: petramond_math::world_pos::WorldPos,
    render_origin: glam::IVec3,
    visual_time: f32,
    proj_y_scale: f32,
}

pub struct Renderer {
    ghosts: ghosts::GhostPass,
    selection: selection::SelectionPass,
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    gpu_timer: Option<gpu_timer::GpuTimer>,
    offscreen_target: Option<(u32, u32, wgpu::TextureView)>,
    sized_frames: Option<Box<sized_frames::SizedFrames>>,
    frame_destination: Option<[u32; 4]>,
    captures: capture::Captures,
    world_marks: world_marks::WorldMarksPass,
    /// The swapchain was rebuilt in response to a suboptimal acquire and came
    /// back STILL suboptimal — stop retrying (some drivers, e.g. NVIDIA on
    /// Wayland, report suboptimal permanently; reconfiguring every frame would
    /// recreate the swapchain at frame rate). Cleared by a good acquire or a
    /// real resize, so genuine size/scale mismatches always get one rebuild.
    suboptimal_retried: bool,
    health: DeviceHealth,
    graph: FrameGraph<Node>,
    frame_plan: FramePlan<Node>,
    binds: SharedBinds,
    model_break: crate::model_break::ModelBreak,
    terrain: TerrainPass,
    view: ViewState,
    targets: SceneTargets,
    chrome: ChromePass,
    sky: SkyPass,
    ui: UiPass,
    hand: HandPass,
    particle: ParticlePass,
    item_entity: ItemEntityPass,
    actor: ActorPass,
    shadow: ShadowPass,
    block_entity: BlockEntityPass,
    last_stats: RenderStats,
}

enum HudLayerTexture {
    Solid,
    Texture(Option<wgpu::BindGroup>),
}

struct HudLayer {
    source: fn(&UiBuild) -> &[UiVertex],
    texture: HudLayerTexture,
    under_chrome: bool,
    vbuf: wgpu::Buffer,
    vertex_count: u32,
}

impl HudLayer {
    fn another(&self, device: &wgpu::Device) -> Self {
        Self {
            source: self.source,
            texture: match &self.texture {
                HudLayerTexture::Solid => HudLayerTexture::Solid,
                HudLayerTexture::Texture(bind) => HudLayerTexture::Texture(bind.clone()),
            },
            under_chrome: self.under_chrome,
            vbuf: dynamic_draw::new_buffer(device, wgpu::BufferUsages::VERTEX, "hud layer vbuf"),
            vertex_count: 0,
        }
    }
}
