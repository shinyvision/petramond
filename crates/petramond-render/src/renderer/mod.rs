use super::gpu_timer;
use crate::camera::{Camera, Containment, Frustum, ViewVolume};
use petramond::world::TerrainRenderHandoff;
use petramond_world::selection::SelectionShape;
use petramond_world::chunk::ChunkPos;

use std::collections::HashMap;
use wgpu::util::DeviceExt;

mod actor_pass;
use actor_pass::{ActorPass, MobGpu, PlayerGpu, VisibleBody};
use skinned_draw::{SkinFrame, SkinnedModel};
mod client_overlay;
mod column_store;
use column_store::{ColumnSlot, ColumnStore};
mod construct;
mod error;
pub use error::{RenderFailure, RenderInitError};
use error::DeviceHealth;
mod ghosts;
mod graph;
use graph::{FrameGraph, FramePlan};
pub use ghosts::GhostPiece;
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
mod ui_frame;
mod upload_queue;
use upload_queue::UploadQueue;

#[cfg(test)]
pub(crate) use construct::instance_descriptor;
pub use construct::new_renderer_from_target;
use dynamic_draw::{DynamicDraw, DynamicInstanceDraw, DynamicVertexDraw};
use icon_atlas::IconAtlas;
use lod::far_leaf_lod_active;
pub use offscreen::new_offscreen_renderer;
pub use offscreen::RenderedFrame;

use super::break_overlay::build_break_overlays;
use super::block_entity_model::push_block_entities;
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
use draw_plan::{QuadPass, SectionOcclusion, TerrainDraws};
use super::selection::outline_vertices;
use super::ui::{build_ui, UiBuild, UiVertex};
use super::uniforms::Uniforms;
use super::{
    BlockEntityInstance, BreakOverlayView, EntityShadow, HeldItemFrame, HeldItemView,
    ItemEntityInstance, MobRenderInstance, ParticleEmitterInstance, ParticleInstance,
    PlayerRenderInstance, RemotePlayerRender, SolidParticleInstance, UiFrame,
};
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

/// Terrain GPU-memory census (see [`Renderer::terrain_memory`]).
#[derive(Copy, Clone, Debug)]
pub struct TerrainMemory {
    /// VRAM the geometry arena reserves.
    pub arena_bytes: u64,
    pub arena_blocks: usize,
    /// Arena bytes reserved but not held by any live column.
    pub arena_free: u64,
    /// Arena bytes handed out to live column layers (size-class rounded).
    pub suballocated: u64,
    /// Of those, the bytes a draw actually reads.
    pub used: u64,
    pub live_allocs: usize,
    /// Fresh suballocations since process start — the churn a sizing policy
    /// trades against.
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
    /// The owning column's slab slot, resolved once by the planner so no
    /// encode loop has to look the column up again.
    column_slot: ColumnSlot,
    opaque_batched: bool,
    model_batched: bool,
    use_far_leaf_lod: bool,
    /// The section's ranges in its column's buffers, by [`SectionStream`]:
    /// the quad passes' draw lists are built from them, and the model passes
    /// draw a section's index ranges when its column is not batched.
    spans: [Span; SectionStream::COUNT],
}

impl VisibleSection {
    #[inline]
    fn span(&self, stream: SectionStream) -> Span {
        self.spans[stream.index()]
    }
}

/// One pack environment (volumetric) pass: its pipeline resources plus the
/// depth-coupled group-0 bind, rebuilt whenever the scene targets are (the
/// bind references the frame depth view).
struct EnvPass {
    res: EnvPassResources,
    bind: wgpu::BindGroup,
    /// True while NONE of the pass's declared params are published (no
    /// session, or the owning mod isn't running): the pass is skipped
    /// entirely — a volumetric with no inputs must cost nothing. A pass
    /// declaring zero params always draws (the author's explicit choice).
    dormant: bool,
}

/// The particle pass: its two draws (cutout block/model cubes and the
/// alpha-blended emitter cubes), the per-frame instances the scene handed
/// over, and the reusable CPU rows both bakes fill.
struct ParticlePass {
    /// Particle cube draw: the particle pipeline, one instance row per
    /// particle expanded to its cube in the vertex stage, as one
    /// [`DynamicInstanceDraw`].
    draw: DynamicInstanceDraw<super::particles::ParticleRow>,
    /// Translucent block-emitter particles: same cube rows as mining dust,
    /// but a separate alpha-blended pipeline/buffer so cutout dust remains unchanged.
    emitter_draw: DynamicInstanceDraw<super::particles::ParticleRow>,
    /// Block-atlas particle cubes to draw this frame.
    instances: Vec<ParticleInstance>,
    /// Model-atlas particle cubes (bbmodel-block flecks) to draw this frame — written into
    /// the SAME particle rows after the block cubes, then drawn with the model atlas bound.
    model_instances: Vec<ParticleInstance>,
    /// Solid-color simulated particles (emitter-burst droplets) joining the
    /// emitter cubes' alpha-blended bake.
    solid_instances: Vec<SolidParticleInstance>,
    /// Loaded block-row particle emitters to synthesize into translucent cube particles.
    /// This frame's VISIBLE emitters — the gather culls, so the bake only
    /// depth-sorts them.
    emitters: Vec<ParticleEmitterInstance>,
    /// See [`Renderer::set_particle_density`].
    density: f32,
    /// Row count of the BLOCK-atlas portion of `draw` this frame (the split
    /// point: instances `[0..this)` draw with the block atlas, the rest with the
    /// model atlas).
    block_count: u32,
    /// Reusable CPU staging for the particle rows.
    rows: Vec<super::particles::ParticleRow>,
    /// Reusable CPU staging for the translucent emitter-particle rows.
    emitter_rows: Vec<super::particles::ParticleRow>,
    /// Reusable generated translucent particles, sorted far-to-near before their rows are written.
    emitter_scratch: Vec<super::particles::TransparentParticleCube>,
}

impl ParticlePass {
    /// Drop every world-scoped particle. Leaving a world must leave nothing
    /// behind that a later frame could draw at stale coordinates.
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

/// The dropped-item pass: three streams (packed block cubes, bbmodel items,
/// extruded sprites) with their per-frame instances, visible subset, and the
/// CPU staging each bake fills.
struct ItemEntityPass {
    /// Item-entity dynamic draw (drawn by the EXISTING opaque pipeline — a cloned
    /// handle — over its OWN buffers, sized separately from animated blocks).
    draw: DynamicDraw,
    verts: Vec<petramond_mesh::Vertex>,
    indices: Vec<u32>,
    /// Reusable scratch for the frustum-visible subset of `instances`.
    visible: Vec<ItemEntityInstance>,
    /// Dropped item-entities to draw in the world this frame.
    instances: Vec<ItemEntityInstance>,
    /// Mod-submitted per-block draw sets for this frame. They share this
    /// pass's stream deliberately: block atlas, CPU-lit, no re-mesh.
    block_draws: Vec<crate::BlockDrawInstance>,
    /// INDICES into `block_draws` that survived THIS frame's camera — the
    /// draw-set twin of `visible`, and separate for the same reason: the
    /// published list is state, so filtering in place would make the next
    /// frame's contents depend on where the camera happened to point during
    /// this one. Indices rather than rows: a re-cull must not pay a refcount
    /// pair and a struct copy per visible set to record a verdict.
    block_draws_visible: Vec<u32>,
    /// Dropped bbmodel item-entities (world-space ItemVertex, model atlas), drawn by the
    /// model pipeline in the model pass — the explicit-UV counterpart of `draw`.
    model_draw: DynamicDraw,
    model_verts: Vec<super::item_model::ItemVertex>,
    model_indices: Vec<u32>,
    /// Dropped SPRITE item-entities extruded into pixel-perfect 3D slabs
    /// (world-space ItemVertex, 2D block atlas — the wall UVs address single
    /// texels), drawn in the item-entity pass on the mob-layout pipeline.
    sprite_draw: DynamicDraw,
    sprite_verts: Vec<super::item_model::ItemVertex>,
    sprite_indices: Vec<u32>,
    /// Per-instance staging for one extruded-sprite build (the builder clears).
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

/// The entity blob-shadow pass: one MULTIPLY-blended ground quad per shadowed
/// entity (mobs, dropped items, bodies). The rows arrive ground-resolved from
/// the presentation gather; this pass only bakes quads and draws them.
struct ShadowPass {
    /// Shadow quad dynamic draw (static per-quad ibuf, grown vbuf).
    draw: DynamicVertexDraw,
    /// Reused CPU staging for the frame's quads.
    verts: Vec<ShadowVertex>,
    /// This frame's shadow rows (world-space), set by the scene adapter.
    instances: Vec<EntityShadow>,
}

impl ShadowPass {
    fn clear_world(&mut self) {
        self.draw.vertex_count = 0;
        self.instances.clear();
    }
}

/// The block-entity pass: every placed animated block (a chest's lid, a
/// door's or trapdoor's swing, a pack's own) drawn from its animated model
/// rather than chunk geometry — one stream, one cull, one bake.
struct BlockEntityPass {
    draw: DynamicDraw,
    /// Animated blocks to draw in the world this frame.
    instances: Vec<BlockEntityInstance>,
    /// Reusable scratch for the frustum-visible subset of `instances`.
    visible: Vec<BlockEntityInstance>,
    /// The visible set the live GPU buffers were baked from, and the render
    /// origin their vertices are relative to.
    ///
    /// Unlike every other dynamic subsystem, this one does not move: an
    /// animated block changes only when it swings, its light changes, or it
    /// comes into view. Everything that can alter a vertex is in the instance
    /// or in the origin, so an unchanged visible set means the buffers already
    /// hold this frame's geometry and the whole build-and-upload is dead work.
    baked: Vec<BlockEntityInstance>,
    baked_origin: glam::IVec3,
}

impl BlockEntityPass {
    fn clear_world(&mut self) {
        self.draw.index_count = 0;
        self.instances.clear();
        self.visible.clear();
        // The buffers no longer describe anything: the next frame must bake.
        self.baked.clear();
        self.baked_origin = glam::IVec3::MIN;
    }
}

/// The terrain pipelines: every draw of packed column geometry, one pipeline
/// per stream the terrain nodes record.
struct TerrainPipes {
    /// Opaque terrain (quantized `TerrainVertex` + column origin instance).
    opaque: crate::pipeline::SampledPipeline,
    /// Translucent BLOCKS (ice): alpha-blended, depth-writing.
    translucent: crate::pipeline::SampledPipeline,
    /// See-through fluid side faces (back faces culled).
    transparent: crate::pipeline::SampledPipeline,
    /// Fluid TOP faces: the transparent pipeline with culling off.
    transparent_two_sided: crate::pipeline::SampledPipeline,
    /// The chunk `ModelVertex` stream (day/night-aware lighting) over the
    /// model atlas.
    world_model: crate::pipeline::SampledPipeline,
    /// The alpha-BLEND twin of `world_model` for the chunk's semi-transparent
    /// bbmodel faces.
    world_model_blend: crate::pipeline::SampledPipeline,
    /// Model→terrain contact shadows (multiplicative, depth read-only, own
    /// coplanar bias).
    contact: crate::pipeline::SampledPipeline,
}

/// This frame's terrain draw plan: what `plan_draw_order` decided, read by
/// the terrain nodes without re-deciding any of it. The vectors are reused
/// frame to frame (capacity retained).
#[derive(Default)]
struct TerrainPlan {
    /// Visible sections still drawing for themselves, sorted near → far.
    /// Transparent terrain stays section-granular; sections wholly covered by
    /// whole-column draws are dropped.
    sections: Vec<VisibleSection>,
    /// Packed columns that draw their whole opaque stream (or its far
    /// region) in one call, near → far.
    opaque_columns: Vec<OpaqueColumnDraw>,
    /// Packed columns that draw their whole model index stream in one call,
    /// near → far.
    model_columns: Vec<(f32, ChunkPos, ColumnSlot)>,
    /// Packed columns with a VISIBLE contact-shadow stream, near → far.
    contact_columns: Vec<(f32, ChunkPos, ColumnSlot)>,
    /// Some visible section carries model faces (the model nodes' gate).
    any_model: bool,
    /// Some visible section carries translucent or fluid faces.
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

/// The terrain pass: the packed per-column GPU geometry, the upload queue
/// that fills it, the pipelines that draw it, and the per-frame draw plan
/// (visible sections and the column runs each node can draw in one call).
struct TerrainPass {
    pipes: TerrainPipes,
    columns: ColumnStore,
    /// Shared instance-step table of per-column world XZ origins, bound once
    /// per terrain pass; each column draw selects its row via `first_instance`.
    column_origins: ColumnOrigins,
    /// Suballocated GPU storage every packed terrain column's geometry lives in.
    geometry: super::resources::TerrainArenas,
    /// Shared index buffer for the implied-triangulation terrain streams.
    quad_index: super::resources::QuadIndexBuffer,
    /// Which dirty columns upload when (see [`UploadQueue`]).
    uploads: UploadQueue,
    plan: TerrainPlan,
    /// The planned quad passes' draw lists, uploaded for indirect submission
    /// (see [`TerrainDraws`]).
    draws: TerrainDraws,
    /// Reusable `(distance, column, index)` keys for the section depth sort,
    /// and the gather buffer the sorted records land in.
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
    /// allocated section list, so the scan used to chase a pointer per column
    /// to read two integers. Rebuilt only when the column set changes.
    cull_index: Vec<ColumnCull>,
    /// [`cull_index`](Self::cull_index) grouped into square regions, each
    /// naming a contiguous run of it.
    cull_regions: Vec<CullRegion>,
    cull_index_revision: u64,
    /// The section visibility graph, refreshed with the cull index, and the
    /// last plan's occlusion flood over it (see [`SectionOcclusion`]).
    occlusion: SectionOcclusion,
}

/// Columns per side of one cull region. A region test rejects up to its square
/// in one AABB test; too small and the regions cost as much as the columns,
/// too large and few regions reject wholly.
const CULL_REGION_SHIFT: i32 = 3;
const CULL_REGION_COLUMNS: i32 = 1 << CULL_REGION_SHIFT;

/// A whole-column opaque draw: `(distance, column, slot, far LOD)`. The flag
/// picks the column's leading far region over its whole opaque stream.
pub(crate) type OpaqueColumnDraw = (f32, ChunkPos, ColumnSlot, bool);

/// One region of [`TerrainPass::cull_index`]: its bounds and the run of column
/// entries it covers (`first..last`).
#[derive(Copy, Clone)]
struct CullRegion {
    cx: i32,
    cz: i32,
    min_cy: i32,
    max_cy: i32,
    first: u32,
    last: u32,
}

/// One column's entry in [`TerrainPass::cull_index`]: everything
/// [`Renderer::column_visible`] reads, and nothing else.
#[derive(Copy, Clone)]
struct ColumnCull {
    pos: ChunkPos,
    slot: ColumnSlot,
    /// The column's installed section span, or `min > max` when it holds none.
    min_cy: i32,
    max_cy: i32,
}

impl TerrainPass {
    /// Drop every column and the plan built over them. The revision bump
    /// invalidates any plan a later frame might otherwise reuse.
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

/// The UI pass: the 2D pipeline every HUD/inventory quad draws with, the
/// document draw path, client overlays, HUD chrome layers, and the icon atlas.
struct UiPass {
    /// UI pipeline (2D HUD / inventory). Every UI quad is drawn with it; group(0)
    /// binds whichever baked texture (or the icon atlas) the quad samples.
    pipe: wgpu::RenderPipeline,
    /// Texture+sampler bind layout used by every UI texture (doc-UI images,
    /// the heart atlas).
    texture_bgl: wgpu::BindGroupLayout,
    /// GUI-document draw path (petramond-ui DrawList upload + batches): every
    /// screen's chrome. See `doc_ui`.
    doc_ui: doc_ui::DocUi,
    /// Client-WASM images drawn directly in physical screen pixels (HUD
    /// overlays and the active modal canvas), outside document layout.
    client_overlays: client_overlay::ClientOverlays,
    /// Solid-color quads (all stack-count digits) packed into one buffer in
    /// draw order: normal counts `[0, counts)`, then tooltip counts, then drag
    /// counts. Drawn with the icon-atlas bind (the solid sentinel skips the
    /// sampler anyway).
    solid_vbuf: wgpu::Buffer,
    /// CPU staging for `solid_vbuf` (cleared + refilled, capacity retained).
    solid_verts: Vec<UiVertex>,
    count_vertex_count: u32,
    overlay_count_vertex_count: u32,
    drag_count_vertex_count: u32,
    /// The HUD chrome layers (hurt vignette, hearts, status effects, …), each
    /// a `UiBuild` vec + texture + vbuf drawn in list order by the UI pass.
    /// A NEW HUD element is one `UiBuild` vec + one [`HudLayer`] entry in
    /// `construct` — not a field trio, upload block, and pass branch each.
    hud_layers: Vec<HudLayer>,
    /// Pre-baked inventory icon atlas (one 64×64 cell per item, rendered once at
    /// init) + its UI-pass bind group + the cell-UV lookup. Every slot icon is now a
    /// 2D textured quad sampling this, not live 3D geometry. See `icon_atlas`.
    icon_atlas: IconAtlas,
    /// Reusable dynamic vbuf for the per-frame icon QUADS (two triangles per filled
    /// slot, sampling the icon atlas), grown to fit.
    icon_quad_vbuf: wgpu::Buffer,
    /// Reusable CPU staging for the per-frame icon-quad vertices (cleared + refilled,
    /// capacity retained — no per-frame allocation).
    icon_quad_verts: Vec<UiVertex>,
    /// Vertex count of the icon quads uploaded this frame (`0` = no icons).
    icon_quad_vertex_count: u32,
    /// Vertex count of the tooltip icon quads appended after normal icons.
    overlay_icon_quad_vertex_count: u32,
    /// Vertex count of the cursor-held icon quads appended after those.
    drag_icon_quad_vertex_count: u32,
    /// Reusable CPU staging for the per-frame UI geometry (all quad buffers +
    /// overlay spans + icon-quad list), cleared + refilled each frame.
    build: UiBuild,
    /// Surface generation used to reject a complete UI frame solved before a
    /// resize, plus the viewport of the most recently prepared coherent UI.
    viewport_generation: u64,
    prepared_viewport: UiViewport,
}

/// The sky + atmosphere pass: the skybox pipeline, the pack environment
/// (volumetric) passes and their half-res machinery, and the fog/sky terms
/// the world passes and the frame clear both read.
struct SkyPass {
    pipe: crate::pipeline::SampledPipeline,
    bind: wgpu::BindGroup,
    texture_bind: wgpu::BindGroup,
    shader_param_keys: Vec<String>,
    light_param_key: Option<String>,
    /// Pack-supplied environment (volumetric) passes in pack load order,
    /// drawn full-screen after all depth-writing world geometry. Usually
    /// empty (zero cost).
    env_passes: Vec<EnvPass>,
    /// Half-res environment machinery: the offscreen colour + depth the env
    /// passes render into, the downsample/composite binds around them, and
    /// the shared scaler pipelines. Rebuilt with the scene targets.
    env_scaler: super::pipeline::EnvScalers,
    env_color: wgpu::TextureView,
    env_depth: wgpu::TextureView,
    env_down_bind: wgpu::BindGroup,
    env_comp_bind: wgpu::BindGroup,
    /// Above-water fog band, derived from the streaming render distance
    /// (`uniforms::fog_range`) via [`Renderer::set_render_distance`] so the fade
    /// always terminates at the loaded-world edge. The end (plus
    /// `TERRAIN_FOG_CULL_PAD`) is also the terrain draw-cull distance.
    fog_start: f32,
    fog_end: f32,
    /// Sim-owned skylight scale (1.0 = identity), mirrored to the CPU lighting
    /// path (`render::lighting::light_rgb`) for items/particles (skinned bodies
    /// light in their vertex stage).
    scale: f32,
    /// Sim-owned sky light colour (white = identity), the CPU mirror of the
    /// `sky_color` uniform lane — applied to the SKY term only.
    color: [f32; 3],
    /// Background clear colour, kept in sync with the fog colour each frame (sky/
    /// biome fog above water, deep blue when submerged) so the horizon matches the
    /// fog the terrain fades into.
    clear_color: [f32; 3],
}

/// The chrome pass: the targeted-block wireframe and the crosshair — screen
/// furniture drawn over the world, each with its own tiny pipeline and a
/// vertex buffer rewritten only when what it draws changes.
struct ChromePass {
    /// Pipeline for the targeted-block wireframe (LineList, black, view_proj only).
    outline_pipe: crate::pipeline::SampledPipeline,
    outline_bind: wgpu::BindGroup,
    /// Line vertices for the selection outline; rewritten only when the selected
    /// target changes (see `selection` / `selection_drawn`).
    outline_vbuf: wgpu::Buffer,
    outline_vertex_count: u32,
    crosshair_pipe: wgpu::RenderPipeline,
    crosshair_vbuf: wgpu::Buffer,
    crosshair_vertex_count: u32,
    crosshair_drawn_size: (u32, u32),
    crosshair_visible: bool,
    /// Currently-targeted outline shape, or None when nothing is targeted.
    selection: Option<SelectionShape>,
    /// The target whose geometry currently sits in `outline_vbuf`, and the
    /// render origin it was baked relative to.
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

/// The offscreen scene targets the world passes render into, plus the
/// post-process pass that takes them to the swapchain (`post_process.rs`
/// owns the mode logic and the target lifecycle). Rebuilt together on resize,
/// so they live together.
struct SceneTargets {
    /// Single-sample scene colour: the world's target with AA Off / SSAA, the
    /// MSAA resolve's destination otherwise; the post-process pass reads it.
    /// Recreated with `depth` on resize.
    scene_color: wgpu::TextureView,
    /// The multisampled colour attachment under MSAA (`None` at one sample);
    /// resolved every frame, never shader-read.
    multisample_color: Option<wgpu::TextureView>,
    /// The device's sample-count ceiling for the scene format.
    max_samples: u32,
    depth: wgpu::TextureView,
    /// World resolution with AA Off (`0.5..=1.0`). Supersampling instead uses
    /// an integer multiple of the native viewport; chrome stays native-res.
    render_scale: f32,
    /// Apply the colour grade during the scene resolve.
    grade_enabled: bool,
    anti_aliasing: petramond::save::client::AntiAliasing,
    grade_pipe: wgpu::RenderPipeline,
    grade_bgl: wgpu::BindGroupLayout,
    grade_bind: wgpu::BindGroup,
    /// The post-process pass's controls, one lane each: `[darken, desat,
    /// sample_axis, grade]` (see `grade.wgsl`).
    post_process_buf: wgpu::Buffer,
    /// The eased mood the buffer currently holds.
    mood: [f32; 2],
}

/// The frame uniforms and the textures many passes bind: one owner, handed
/// to every node's recording through its pass context.
struct SharedBinds {
    /// The frame [`Uniforms`], rewritten by `update_uniforms`.
    uniform_buf: wgpu::Buffer,
    /// The sky shader's pack parameter block.
    shader_params_buf: wgpu::Buffer,
    /// group(0) of the world pipelines: the uniforms + the uv-rect table.
    uniform: wgpu::BindGroup,
    /// The 2D block atlas (sprites, cracks, particles).
    atlas: wgpu::BindGroup,
    /// The terrain tile-ARRAY (group 1 of the block pipelines), parallel to
    /// `atlas`: block terrain binds this, everything else the 2D atlas.
    atlas_array: wgpu::BindGroup,
    /// The combined bbmodel-block atlas.
    model_atlas: wgpu::BindGroup,
}

/// The camera-derived view state refreshed once per frame in
/// `update_uniforms` and read by every cull and sort.
struct ViewState {
    /// Camera frustum for viewspace culling, refreshed each frame in
    /// `update_uniforms`; chunk meshes outside it are skipped in `render`.
    frustum: Frustum,
    /// Camera world position, refreshed in `update_uniforms`; used to sort
    /// chunk draws front-to-back (opaque) / back-to-front (transparent).
    cam_pos: petramond_math::world_pos::WorldPos,
    /// Snapped integer world origin every world draw is positioned relative to,
    /// keeping GPU transform math camera-local far from spawn.
    render_origin: glam::IVec3,
    /// Visual time from the current frame uniforms, used by presentation-only
    /// render effects such as block-row particle emitters.
    visual_time: f32,
    /// The projection's vertical scale (`1 / tan(fov_y / 2)`), refreshed with
    /// the frustum so a widened FOV narrows what the gathers consider visible
    /// in the same frame it widens the view.
    proj_y_scale: f32,
}

pub struct Renderer {
    ghosts: ghosts::GhostPass,
    selection: selection::SelectionPass,
    /// The presentation swapchain, or `None` for a surfaceless renderer (see
    /// `offscreen`) that draws into its own texture and never presents.
    /// `config` describes the frame geometry + colour format either way.
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// Opt-in per-pass GPU timing (`PETRAMOND_GPU_TIMING=1`); `None` normally.
    gpu_timer: Option<gpu_timer::GpuTimer>,
    /// Reusable colour target for repeated surfaceless frames (tooling); built
    /// on first use so a windowed renderer never allocates it.
    offscreen_target: Option<(u32, u32, wgpu::TextureView)>,
    /// The swapchain was rebuilt in response to a suboptimal acquire and came
    /// back STILL suboptimal — stop retrying (some drivers, e.g. NVIDIA on
    /// Wayland, report suboptimal permanently; reconfiguring every frame would
    /// recreate the swapchain at frame rate). Cleared by a good acquire or a
    /// real resize, so genuine size/scale mismatches always get one rebuild.
    suboptimal_retried: bool,
    /// Device loss and uncaptured GPU errors, as reported by the device's
    /// callbacks (see [`Renderer::failure`]).
    health: DeviceHealth,
    /// Every pass the frame can record, and the reusable plan the graph
    /// derives from them each frame (see `passes`).
    graph: FrameGraph<Node>,
    frame_plan: FramePlan<Node>,
    /// The frame uniforms and the texture binds several passes share.
    binds: SharedBinds,
    /// The bbmodel-block break crack: pipeline + per-frame mask uniform + the
    /// columns whose model streams the decal pass re-draws.
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

/// What a [`HudLayer`] samples.
enum HudLayerTexture {
    /// Solid-color quads: the solid sentinel skips the sampler, so the layer
    /// draws with the icon-atlas bind (any layout-compatible bind works).
    Solid,
    /// The layer's own texture bind, or `None` when its art failed to load —
    /// the layer then draws nothing.
    Texture(Option<wgpu::BindGroup>),
}

/// One HUD chrome layer of the UI pass: a `UiBuild` vertex list uploaded to
/// its own buffer and drawn with its own texture. Layers draw in list order;
/// `under_chrome` ones go beneath the GUI-document draw list (the hurt
/// vignette), the rest above it (hearts, status effects).
struct HudLayer {
    /// Which `UiBuild` vec fills this layer each frame.
    source: fn(&UiBuild) -> &[UiVertex],
    texture: HudLayerTexture,
    /// Draw beneath the GUI-document chrome instead of over it.
    under_chrome: bool,
    vbuf: wgpu::Buffer,
    vertex_count: u32,
}
