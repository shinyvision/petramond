use super::gpu_timer;
use crate::camera::{Camera, Frustum, ViewVolume};
use petramond::world::TerrainRenderHandoff;
use petramond_math::math::SelectionShape;
use petramond_world::chunk::{ChunkPos, SectionPos};

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use wgpu::util::DeviceExt;

mod client_overlay;
mod construct;
mod doc_ui;
mod dynamic_bake;
mod dynamic_draw;
mod frame_state;
mod icon_atlas;
mod lod;
mod offscreen;
mod passes;
mod ui_frame;

#[cfg(test)]
pub(crate) use construct::instance_descriptor;
pub use construct::new_renderer_from_target;
use dynamic_draw::{DynamicDraw, DynamicVertexDraw};
use icon_atlas::IconAtlas;
use lod::far_leaf_lod_active;
pub use offscreen::new_offscreen_renderer;
pub use offscreen::RenderedFrame;

use super::break_overlay::build_break_overlays;
use super::chest_model::build_chests;
use super::crosshair::crosshair_vertices;
use super::door_model::build_doors;
use super::entity_shadow::{build_entity_shadows, ShadowVertex};
use super::hand::build_hand_lit;
use super::hand_animator::HeldItemAnimator;
use super::item_entity::build_item_entities;
use super::item_model::ItemVertex;
use super::mob_model::build_mob_instances;
use super::particles::{build_particles_split, build_transparent_emitter_particles};
use super::pipeline::{create_pipeline_resources, EnvPassResources};
use super::resources::{
    create_atlas, create_atlas_array, create_depth, create_gui_panel, create_model_texture,
    create_scene_color, upload_column_mesh, ColumnOrigins, ColumnUploadScratch, GpuColumnMesh,
    GpuSectionMesh,
};
use super::selection::outline_vertices;
use super::ui::{build_ui, UiBuild, UiVertex};
use super::uniforms::{Uniforms, UNDERWATER_FOG_END, UNDERWATER_FOG_START};
use super::{
    BreakOverlayView, ChestInstance, DoorInstance, EntityShadow, HeldItemFrame, HeldItemView,
    ItemEntityInstance, MobRenderInstance, ParticleEmitterInstance, ParticleInstance,
    PlayerRenderInstance, RemotePlayerRender, SolidParticleInstance, UiFrame,
};
use petramond::gui::{UiSnapshot, UiViewport};
use petramond_world::bbmodel::Model;

const TERRAIN_FOG_CULL_PAD: f32 = 32.0;

#[derive(Clone, PartialEq, Eq)]
struct TerrainViewKey {
    view_proj: [u32; 16],
    cam: [u32; 3],
    fog: u32,
}

struct PendingTerrainUpload {
    revision: u64,
    quiet_after: u64,
    deadline: u64,
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
    opaque_batched: bool,
    model_batched: bool,
    use_far_leaf_lod: bool,
    /// Base vertex + quad count of the section's implied-triangulation opaque
    /// streams (see [`petramond_mesh::QuadIdx`]).
    opaque_vertex_start: u32,
    opaque_quads: u32,
    far_opaque_vertex_start: u32,
    far_opaque_quads: u32,
    transparent_vertex_start: u32,
    transparent_quads: u32,
    transparent_ts_vertex_start: u32,
    transparent_ts_quads: u32,
    translucent_vertex_start: u32,
    translucent_quads: u32,
    model_index_start: u32,
    model_idx_count: u32,
    /// The section's alpha-blend model face range in the column index buffer
    /// (drawn by the model-blend pass; see [`petramond_mesh::ChunkMesh::model_blend_idx`]).
    model_blend_index_start: u32,
    model_blend_idx_count: u32,
}

/// Per-species GPU resources for the mob pipeline, built once at renderer init by
/// iterating [`petramond::mob::defs()`] (so the renderer never names a species). Borrows
/// the species' precached [`Model`] + its render scale, the species' own texture/sampler + group(1)
/// bind, its dynamic draw buffers, and reused per-frame scratch (the visible subset
/// + the baked `ItemVertex` geometry). The `Vec<MobGpu>` is in `Mob as usize` order.
struct MobGpu {
    model: &'static Model,
    scale: f32,
    bind: wgpu::BindGroup,
    draw: DynamicDraw,
    /// Live-mob frustum cull volume around the instance position, derived from
    /// the REST-POSED model bounds × scale plus animation slack (see
    /// `construct`): horizontal radius (yaw-independent — the farthest posed
    /// corner can point any way) and the vertical extent relative to the feet.
    /// A hardcoded pad was the old bug: it topped out at 1.2 m and clipped any
    /// taller species (the hushjaw is ~1.9 m) out of the frustum early.
    cull_r: f32,
    cull_y0: f32,
    cull_y1: f32,
    /// Frustum-visible subset of this species' instances this frame.
    visible: Vec<MobRenderInstance>,
    /// Reused CPU staging for this species' baked geometry.
    verts: Vec<ItemVertex>,
    indices: Vec<u32>,
}

/// GPU resources for player bodies — the local third-person body AND every
/// remote player, all sharing the precached player model + skin texture bind
/// (per-remote skins are out of scope). One dynamic draw over the shared mob
/// pipeline; `verts`/`indices` are the COMBINED per-frame staging every
/// visible body appends into.
struct PlayerGpu {
    model: &'static Model,
    bind: wgpu::BindGroup,
    draw: DynamicDraw,
    verts: Vec<ItemVertex>,
    indices: Vec<u32>,
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
/// over, and the reusable CPU staging both bakes fill.
struct ParticlePass {
    /// Particle billboard draw: the particle pipeline + a per-frame vbuf and a
    /// STATIC quad ibuf, as one [`DynamicVertexDraw`].
    draw: DynamicVertexDraw,
    /// Translucent block-emitter particles: same cube vertex format as mining dust,
    /// but a separate alpha-blended pipeline/vbuf so cutout dust remains unchanged.
    emitter_draw: DynamicVertexDraw,
    /// Block-atlas particle cubes to draw this frame.
    instances: Vec<ParticleInstance>,
    /// Model-atlas particle cubes (bbmodel-block flecks) to draw this frame — baked into
    /// the SAME particle vbuf after the block cubes, then drawn with the model atlas bound.
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
    /// Vertex count of the BLOCK-atlas portion of `draw` this frame (the split
    /// point: `[0..this)` draws with the block atlas, the rest with the model atlas).
    block_vertex_count: u32,
    /// Reusable CPU staging for baked particle vertices.
    verts: Vec<super::particles::ParticleVertex>,
    /// Reusable CPU staging for translucent emitter-particle vertices.
    emitter_verts: Vec<super::particles::ParticleVertex>,
    /// Reusable generated translucent particle rows, sorted far-to-near before vertex bake.
    emitter_scratch: Vec<super::particles::TransparentParticleCube>,
}

impl ParticlePass {
    /// Drop every world-scoped particle. Leaving a world must leave nothing
    /// behind that a later frame could draw at stale coordinates.
    fn clear_world(&mut self) {
        self.draw.vertex_count = 0;
        self.emitter_draw.vertex_count = 0;
        self.block_vertex_count = 0;
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
    /// handle — over its OWN fixed-size buffers, sized separately from chests).
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

/// The actor pass: every animated body (mobs, the local third-person player,
/// remote players) and the held-item streams attached to their hands.
struct ActorPass {
    /// Per-species mob render resources, indexed by `Mob as usize` (registry id
    /// order). Built once from `mob::defs()`; each frame the visible mobs are
    /// grouped here by species, baked, and drawn in the mob pass.
    mob_gpu: Vec<MobGpu>,
    /// Mobs to draw in the world this frame (the scene adapter fills this by
    /// interpolating the sim's live mob instances).
    mobs: Vec<MobRenderInstance>,
    /// Player-body resources (local third-person + remote players, one
    /// combined stream drawn in the mob pass).
    player_gpu: PlayerGpu,
    /// The LOCAL third-person body to draw this frame (`None` in first person).
    player_view: Option<PlayerRenderInstance>,
    /// The remote players' bodies + held-item views for this frame.
    remote_players: Vec<RemotePlayerRender>,
    /// Frustum-visible bodies this frame (local first, then remotes), each
    /// paired with the held-item view that animates its hand.
    player_visible: Vec<(PlayerRenderInstance, HeldItemView, HeldItemView)>,
    /// Per-body staging for one `build_player_body` bake, appended into
    /// `player_gpu`'s combined stream.
    body_verts: Vec<super::item_model::ItemVertex>,
    body_indices: Vec<u32>,
    /// Held EXTRUDED-SPRITE items across all bodies (explicit-UV stream, 2D
    /// atlas), attached to each posed right hand.
    item_draw: DynamicDraw,
    item_verts: Vec<super::item_model::ItemVertex>,
    item_indices: Vec<u32>,
    /// Per-item staging for one extruded-sprite build (the builder clears).
    sprite_verts: Vec<super::item_model::ItemVertex>,
    /// Held BBMODEL items across all bodies (explicit-UV stream, MODEL atlas) —
    /// split from the sprite stream so mixed hands draw with the right texture.
    model_item_draw: DynamicDraw,
    model_item_verts: Vec<super::item_model::ItemVertex>,
    model_item_indices: Vec<u32>,
    /// Held BLOCK mini-cubes across all bodies (packed block vertices, opaque
    /// pipeline + terrain atlas array), CPU-transformed to each hand.
    block_item_draw: DynamicDraw,
}

impl ActorPass {
    fn clear_world(&mut self) {
        for mob in &mut self.mob_gpu {
            mob.draw.index_count = 0;
            mob.visible.clear();
        }
        self.mobs.clear();
        self.player_gpu.draw.index_count = 0;
        self.player_view = None;
        self.remote_players.clear();
        self.player_visible.clear();
        self.item_draw.index_count = 0;
        self.model_item_draw.index_count = 0;
        self.block_item_draw.index_count = 0;
    }
}

/// The block-entity pass: placed blocks drawn as animated models rather than
/// chunk geometry (chest lids, door swings), each with its own draw caps so a
/// wall of one cannot starve the other.
struct BlockEntityPass {
    chest_draw: DynamicDraw,
    /// Placed chests to draw in the world this frame.
    chests: Vec<ChestInstance>,
    /// Reusable scratch for the frustum-visible subset of `chests`.
    chest_visible: Vec<ChestInstance>,
    door_draw: DynamicDraw,
    /// Placed doors to draw in the world this frame.
    doors: Vec<DoorInstance>,
    /// Reusable scratch for the frustum-visible subset of `doors`.
    door_visible: Vec<DoorInstance>,
}

impl BlockEntityPass {
    fn clear_world(&mut self) {
        self.chest_draw.index_count = 0;
        self.door_draw.index_count = 0;
        self.chests.clear();
        self.chest_visible.clear();
        self.doors.clear();
        self.door_visible.clear();
    }
}

/// The terrain pass: the packed per-column GPU geometry, the persistent upload
/// queue that fills it, and the per-frame draw plan (visible sections and the
/// column runs each pass can draw in one call).
/// A terrain upload's heap ordering key: priority band, then the frame it was
/// queued on, then the column, then a tiebreak sequence — so equal-priority
/// columns retire in the order they arrived.
type UploadKey = (u8, u32, i32, i32, u64);

struct TerrainPass {
    columns: HashMap<ChunkPos, GpuColumnMesh>,
    /// Shared instance-step table of per-column world XZ origins, bound once
    /// per terrain pass; each column draw selects its row via `first_instance`.
    column_origins: ColumnOrigins,
    /// Suballocated GPU storage every packed terrain column's geometry lives in.
    geometry: super::geometry_arena::GeometryArena,
    /// Shared index buffer for the implied-triangulation terrain streams.
    quad_index: super::resources::QuadIndexBuffer,
    /// Persistent upload work. World dirtiness is level-triggered, so the set
    /// deduplicates columns while the heap preserves their first useful priority.
    upload_pending: HashMap<ChunkPos, PendingTerrainUpload>,
    upload_heap: BinaryHeap<Reverse<UploadKey>>,
    upload_frame: u64,
    /// Reusable CPU staging for packing section meshes into a GPU column upload.
    upload_scratch: ColumnUploadScratch,
    /// Reusable per-frame section draw order, sorted near→far. Transparent terrain
    /// stays section-granular; opaque/model passes can mark sections covered by a single
    /// packed column draw.
    draw_order: Vec<VisibleSection>,
    /// Reusable near→far list of packed columns that can draw their whole opaque index
    /// stream in one call this frame.
    opaque_column_order: Vec<(f32, ChunkPos)>,
    /// Reusable near→far list of packed columns that can draw their whole model index
    /// stream in one call this frame.
    model_column_order: Vec<(f32, ChunkPos)>,
    /// Reusable near→far list of packed columns with a VISIBLE contact-shadow
    /// stream this frame.
    contact_column_order: Vec<(f32, ChunkPos)>,
    gpu_revision: u64,
    planned_gpu_revision: u64,
    view_key: TerrainViewKey,
    planned_view_key: Option<TerrainViewKey>,
    plan_any_model: bool,
    plan_any_transparent: bool,
    /// Sections currently drawing the far leaf mesh. Stored only for active far-LOD
    /// sections so the transition has hysteresis instead of flipping at one threshold.
    far_leaf_lod_state: HashMap<SectionPos, bool>,
}

impl TerrainPass {
    /// Drop every column and the plan built over them. The revision bump
    /// invalidates any plan a later frame might otherwise reuse.
    fn clear_world(&mut self) {
        self.columns.clear();
        self.upload_pending.clear();
        self.upload_heap.clear();
        self.gpu_revision = self.gpu_revision.wrapping_add(1);
        self.planned_view_key = None;
        self.far_leaf_lod_state.clear();
        self.draw_order.clear();
        self.opaque_column_order.clear();
        self.model_column_order.clear();
        self.contact_column_order.clear();
    }
}

/// The first-person hand pass: the held item's own pipelines and buffers,
/// the per-frame hand geometry, and the break-crack decal drawn with it.
struct HandPass {
    /// Depth-enabled model3d variant for the first-person held block in the hand
    /// pass (same shader; the hand pass clears depth so the held block self-sorts).
    /// (The depthless `model3d_pipe` is now used only to bake the icon atlas at init,
    /// so it isn't stored here.)
    model3d_pipe: wgpu::RenderPipeline,
    /// Dynamic-offset MVP uniform buffer (256-byte slots); slot 0 is the hand.
    model3d_mvp_buf: wgpu::Buffer,
    /// group(0) bind for model3d (MVP at binding 0 + uv_rects at binding 1).
    model3d_mvp_bind: wgpu::BindGroup,
    /// Reusable dynamic vertex/index buffers for model3d draws (rewritten in place).
    model3d_vbuf: wgpu::Buffer,
    model3d_ibuf: wgpu::Buffer,
    /// item3d pipeline (extruded first-person held item) + its group0 MVP bind
    /// (over the shared `model3d_mvp_buf`, slot 0) and reusable dynamic vbuf.
    item3d_pipe: wgpu::RenderPipeline,
    item3d_mvp_bind: wgpu::BindGroup,
    item3d_vbuf: wgpu::Buffer,
    /// Reusable CPU staging for the extruded held-item geometry (cleared +
    /// refilled by `item_model::build_extruded_item`, capacity retained).
    item3d_verts: Vec<super::item_model::ItemVertex>,
    /// Vertex count of the extruded held item uploaded this frame (0 = none).
    item3d_vertex_count: u32,
    /// True when this frame's item3d geometry is a held bbmodel block (drawn with the
    /// MODEL atlas) rather than an extruded sprite (the block atlas).
    held_is_model: bool,
    /// Index count of the hand geometry uploaded for this frame (0 = nothing).
    index_count: u32,
    /// Vertex count of the hand geometry — the OFF-hand geometry appends
    /// after it in the shared model3d vbuf, so its `base_vertex` starts here.
    vertex_count: u32,
    // --- The OFF (left) hand: its own view/animator, its geometry appended
    // --- into the SAME buffers after the main hand's, MVP slot 1. Drawn only
    // --- while the off-hand slot holds an item (no bare left arm).
    /// Off-hand held item state (`item == None` = empty, nothing drawn).
    off_item: HeldItemView,
    off_item_anim: HeldItemAnimator,
    /// Index count of the off-hand model3d geometry (drawn at
    /// `index_count..index_count + off_index_count` with `base_vertex =
    /// vertex_count`).
    off_index_count: u32,
    /// The off-hand item3d stream's `[start, start + count)` vertex range in
    /// the shared item3d vbuf (appended after the main hand's stream).
    off_item3d_start: u32,
    off_item3d_count: u32,
    /// The off item3d stream draws with the MODEL atlas (bbmodel) rather than
    /// the block atlas (extruded sprite) — per-stream twin of `held_is_model`.
    off_is_model: bool,
    /// Reusable CPU staging for the per-frame hand geometry (cleared + refilled by
    /// `build_hand`, capacity retained — no per-frame allocation).
    verts: Vec<petramond_mesh::Vertex>,
    indices: Vec<u32>,
    /// Break-overlay (destroy crack): its own pipeline + dynamic vbuf/ibuf + the
    /// index count baked this frame (0 = no overlay), as one [`DynamicDraw`].
    break_draw: DynamicDraw,
    // --- Per-frame view state handed off by the App, drawn in `render`. ---
    /// Block-break overlays to draw this frame (own + capped remotes; empty =
    /// none).
    break_overlays: Vec<BreakOverlayView>,
    /// First-person held item / hand state (defaults to the bare hand).
    held_item: HeldItemView,
    visible: bool,
    /// Screen-space (NDC) offset applied to the whole hand/held-item draw this
    /// frame — the hurt-shake jitter. Zero when calm.
    shake: [f32; 2],
    held_item_anim: HeldItemAnimator,
    held_item_skylight: u8,
    held_item_blocklight: petramond_world::light::BlockLight6,
}

impl HandPass {
    /// Drop the world-scoped hand state. The held item and its animator
    /// are world state too — a stale pose must not survive into the next.
    fn clear_world(&mut self) {
        self.visible = false;
        self.index_count = 0;
        self.vertex_count = 0;
        self.item3d_vertex_count = 0;
        self.held_is_model = false;
        self.held_item = HeldItemView::default();
        self.held_item_anim = HeldItemAnimator::default();
        self.off_item = HeldItemView::default();
        self.off_item_anim = HeldItemAnimator::default();
        self.off_index_count = 0;
        self.off_item3d_start = 0;
        self.off_item3d_count = 0;
        self.off_is_model = false;
        self.shake = [0.0; 2];
        self.break_overlays.clear();
        self.break_draw.index_count = 0;
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
    /// slot, sampling the icon atlas). Grown to fit if a frame ever exceeds it (never
    /// a hard cap that would drop the whole batch).
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
    pipe: wgpu::RenderPipeline,
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
    env_scaler: super::pipeline::EnvScaler,
    env_color: wgpu::TextureView,
    env_depth: wgpu::TextureView,
    env_down_bind: wgpu::BindGroup,
    env_comp_bind: wgpu::BindGroup,
    underwater: bool,
    /// Above-water fog band, derived from the streaming render distance
    /// (`uniforms::fog_range`) via [`Renderer::set_render_distance`] so the fade
    /// always terminates at the loaded-world edge. The end (plus
    /// `TERRAIN_FOG_CULL_PAD`) is also the terrain draw-cull distance.
    fog_start: f32,
    fog_end: f32,
    /// Sim-owned skylight scale (1.0 = identity), mirrored to the CPU lighting
    /// path (`render::lighting::light_rgb`) for mobs/items/particles.
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
    outline_pipe: wgpu::RenderPipeline,
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
    /// The target whose geometry currently sits in `outline_vbuf`.
    selection_drawn: Option<SelectionShape>,
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

/// The offscreen scene targets the world passes render into, plus the grade
/// pass that resolves them to the swapchain. Rebuilt together on resize, so
/// they live together.
struct SceneTargets {
    /// Offscreen scene-colour target the world passes render into; the grade
    /// pass reads it and writes the swapchain. Recreated with `depth` on resize.
    scene_color: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// Internal resolution scale for the world passes (`0.5..=1.0`): scene_color
    /// and depth are created at `swapchain × scale` and the grade pass upscales.
    /// Fill-rate knob for weak GPUs; chrome (UI/crosshair) stays native-res.
    render_scale: f32,
    /// When false (and `render_scale == 1.0`), the world renders straight into
    /// the swapchain and the grade pass + offscreen round-trip are skipped.
    grade_enabled: bool,
    grade_pipe: wgpu::RenderPipeline,
    grade_bgl: wgpu::BindGroupLayout,
    grade_bind: wgpu::BindGroup,
    /// Mod-mood uniform (grade pass binding 2): `[darken, desat, 0, 0]`.
    mood_buf: wgpu::Buffer,
    /// The eased mood the buffer currently holds.
    mood: [f32; 2],
}

/// The camera-derived view state refreshed once per frame in
/// `update_uniforms` and read by every cull and sort.
struct ViewState {
    /// Camera frustum for viewspace culling, refreshed each frame in
    /// `update_uniforms`; chunk meshes outside it are skipped in `render`.
    frustum: Frustum,
    /// Camera world position, refreshed in `update_uniforms`; used to sort
    /// chunk draws front-to-back (opaque) / back-to-front (transparent).
    cam_pos: glam::Vec3,
    /// Snapped world-space origin subtracted by world shaders before applying the
    /// camera matrix, keeping GPU transform math camera-local far from spawn.
    render_origin: glam::Vec3,
    /// Visual time from the current frame uniforms, used by presentation-only
    /// render effects such as block-row particle emitters.
    visual_time: f32,
}

pub struct Renderer {
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
    opaque_pipe: wgpu::RenderPipeline,
    translucent_pipe: wgpu::RenderPipeline,
    /// Water TOP faces: the transparent pipeline with culling off.
    transparent_two_sided_pipe: wgpu::RenderPipeline,
    transparent_pipe: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    shader_params_buf: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    atlas_bind: wgpu::BindGroup,
    /// Terrain tile-ARRAY bind (group 1 for the opaque/transparent block pipelines),
    /// parallel to `atlas_bind`; the block terrain draws bind this, everything else the 2D atlas.
    atlas_array_bind: wgpu::BindGroup,
    /// bbmodel-block ("model") render resources: the mob pipeline reused for the model
    /// pass plus the combined model atlas bound at group(1). The geometry itself lives
    /// in packed terrain columns as per-section model ranges, so there's no per-frame
    /// model bake — the model pass just draws the visible sections' model streams.
    model_pipe: wgpu::RenderPipeline,
    /// Pipeline for the chunk `ModelVertex` stream (day/night-aware lighting);
    /// `model_pipe` (mob layout) keeps drawing dropped bbmodel item entities.
    world_model_pipe: wgpu::RenderPipeline,
    /// The alpha-BLEND twin of `world_model_pipe` for the chunk's
    /// semi-transparent bbmodel faces; draws in the model-blend pass after the
    /// translucent-block pass.
    world_model_blend_pipe: wgpu::RenderPipeline,
    /// Model→terrain contact-shadow pipeline (multiplicative, depth read-only,
    /// own coplanar bias); draws the packed columns' contact streams between
    /// the opaque and sky passes.
    contact_pipe: wgpu::RenderPipeline,
    model_atlas_bind: wgpu::BindGroup,
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

impl Renderer {
    /// Couple the fog band (and with it the terrain draw-cull distance) to the
    /// streaming render distance, so the fade always ends at the loaded edge.
    pub fn set_render_distance(&mut self, chunks: i32) {
        let (start, end) = super::uniforms::fog_range(chunks);
        self.sky.fog_start = start;
        self.sky.fog_end = end;
    }

    /// Terrain draw-cull distance: nothing beyond this is fully un-fogged.
    pub(crate) fn terrain_cull_dist(&self) -> f32 {
        self.sky.fog_end + TERRAIN_FOG_CULL_PAD
    }

    /// What this frame can draw, as published by the last
    /// [`update_uniforms`](Self::update_uniforms): the culling frustum and the
    /// fog cull distance. Hand it to a per-frame gather so the gather's cost
    /// tracks what is visible instead of what is loaded.
    pub fn view_volume(&self) -> ViewVolume {
        ViewVolume::new(
            self.view.frustum,
            self.view.render_origin,
            self.view.cam_pos,
            self.terrain_cull_dist(),
        )
    }

    /// Emitter-derived particle density from the particles graphics option
    /// (`0` = off, `0.5` = reduced, `1` = full). Scales each looping emitter's
    /// active-particle count; zero skips emitter baking entirely.
    pub fn set_particle_density(&mut self, density: f32) {
        self.particle.density = density.clamp(0.0, 1.0);
    }

    /// Set the internal world-resolution scale (clamped `0.5..=1.0`) and rebuild
    /// the offscreen targets. The grade pass upscales to the swapchain.
    pub fn set_render_scale(&mut self, scale: f32) {
        let scale = scale.clamp(0.5, 1.0);
        if (scale - self.targets.render_scale).abs() < f32::EPSILON {
            return;
        }
        self.targets.render_scale = scale;
        self.recreate_scene_targets();
    }

    /// Toggle the colour-grade pass. Off (at native scale) skips the offscreen
    /// scene round-trip entirely — the world renders straight to the swapchain.
    pub fn set_grade_enabled(&mut self, on: bool) {
        self.targets.grade_enabled = on;
    }

    /// World passes bypass the offscreen target only when nothing needs it:
    /// grade off AND native scale (upscaling needs the small target + grade).
    pub(crate) fn direct_to_swapchain(&self) -> bool {
        !self.targets.grade_enabled && self.targets.render_scale >= 1.0
    }

    /// The offscreen scene/depth dimensions under `render_scale`.
    pub(crate) fn scene_dims(&self) -> (u32, u32) {
        let scale = self.targets.render_scale;
        (
            ((self.config.width as f32 * scale).round() as u32).max(1),
            ((self.config.height as f32 * scale).round() as u32).max(1),
        )
    }

    /// Mean GPU nanoseconds per pass over the frames measured since the last
    /// [`Renderer::reset_gpu_profile`], as `(label, total_ns, frames)`. Empty
    /// unless `PETRAMOND_GPU_TIMING` is set.
    pub fn gpu_profile(&self) -> Vec<(&'static str, f64, u32)> {
        self.gpu_timer
            .as_ref()
            .map(|t| t.report())
            .unwrap_or_default()
    }

    /// Where the packed terrain columns' GPU memory actually is. The
    /// renderer's dominant VRAM consumer at high render distance, and the
    /// number that says whether an allocation-policy change paid.
    pub fn terrain_memory(&self) -> TerrainMemory {
        let mut suballocated = 0u64;
        let mut live_allocs = 0usize;
        let mut used = 0u64;
        for col in self.terrain.columns.values() {
            for b in [
                &col.opaque_vbuf,
                &col.far_opaque_vbuf,
                &col.transparent_vbuf,
                &col.transparent_ts_vbuf,
                &col.translucent_vbuf,
                &col.model_vbuf,
                &col.model_ibuf,
                &col.contact_vbuf,
            ]
            .into_iter()
            .flatten()
            {
                suballocated += b.alloc.capacity();
                used += b.len;
                live_allocs += 1;
            }
        }
        TerrainMemory {
            arena_bytes: self.terrain.geometry.reserved_bytes(),
            arena_free: self.terrain.geometry.free_bytes(),
            arena_blocks: self.terrain.geometry.block_count(),
            suballocated,
            used,
            live_allocs,
            suballocs_since_start: super::resources::TERRAIN_SUBALLOCS
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// `(bytes, count)` of every GPU texture created this process (see
    /// [`crate::gpu_mem`]). Gross, not net: resize-replaced targets are
    /// counted each time.
    pub fn texture_memory(&self) -> (u64, u64) {
        super::gpu_mem::texture_totals()
    }

    /// Texture bytes per descriptor label, largest first.
    pub fn texture_memory_by_label(&self) -> Vec<(String, u64)> {
        super::gpu_mem::texture_by_label()
    }

    /// Terrain draw work submitted by the last encoded frame:
    /// `(opaque draws, opaque indices, transparent draws, transparent indices)`.
    pub fn last_terrain_draws(&self) -> (u32, u64, u32, u64) {
        let s = self.last_stats;
        (
            s.opaque_draws,
            s.opaque_indices,
            s.transparent_draws,
            s.transparent_indices,
        )
    }

    /// Mean CPU nanoseconds per frame stage, same shape as [`Renderer::gpu_profile`].
    pub fn cpu_profile(&self) -> Vec<(&'static str, f64, u32)> {
        self.gpu_timer
            .as_ref()
            .map(|t| t.report_cpu())
            .unwrap_or_default()
    }

    pub fn reset_gpu_profile(&self) {
        if let Some(t) = &self.gpu_timer {
            t.reset();
        }
    }

    pub fn render(&mut self) {
        let Some(frame) = self.acquire_swapchain_frame() else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.encode_frame(&view);
        frame.present();
    }

    /// The swapchain image to draw into, or `None` when this frame draws
    /// nothing: a surfaceless renderer, or a swapchain that needed rebuilding
    /// first.
    fn acquire_swapchain_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        let surface = self.surface.as_ref()?;
        match surface.get_current_texture() {
            // A suboptimal frame still presents (with a per-present driver
            // warning), but the swapchain no longer matches the surface —
            // rebuild it once and draw from the fresh one next frame. The
            // frame must drop BEFORE the reconfigure (a live SurfaceTexture
            // across a swapchain rebuild panics).
            Ok(t) if t.suboptimal && !self.suboptimal_retried => {
                self.suboptimal_retried = true;
                drop(t);
                surface.configure(&self.device, &self.config);
                None
            }
            Ok(t) => {
                self.suboptimal_retried = t.suboptimal;
                Some(t)
            }
            // Stale/lost swapchain (a resize or compositor change the events
            // haven't delivered yet): reconfigure at the current size and let
            // the next frame draw.
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                surface.configure(&self.device, &self.config);
                None
            }
            Err(_) => None,
        }
    }

    /// Everything between "here is the colour target" and "the GPU has this
    /// frame": the per-frame CPU bakes, draw planning, pass encoding, submit.
    /// Target-agnostic, so the windowed swapchain and an offscreen capture
    /// share one frame graph.
    fn encode_frame(&mut self, view: &wgpu::TextureView) {
        let mark = std::time::Instant::now;
        let t = mark();
        self.refresh_overlay_buffers();
        self.prepare_held_item();
        self.bake_world_instances();
        if let Some(g) = &self.gpu_timer {
            g.cpu_stage("cpu: bake world instances", t.elapsed().as_nanos() as f64);
        }

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        // Reusable draw orders taken out so `plan_draw_order` can fill them while
        // `self` is read; restored after encoding (capacity retained next frame).
        let mut order = std::mem::take(&mut self.terrain.draw_order);
        let mut opaque_columns = std::mem::take(&mut self.terrain.opaque_column_order);
        let mut model_columns = std::mem::take(&mut self.terrain.model_column_order);
        let mut contact_columns = std::mem::take(&mut self.terrain.contact_column_order);
        let t = mark();
        let (mut stats, any_model_visible, any_transparent_visible) = self.plan_draw_order(
            &mut order,
            &mut opaque_columns,
            &mut model_columns,
            &mut contact_columns,
        );
        if let Some(g) = &self.gpu_timer {
            g.cpu_stage("cpu: plan draw order", t.elapsed().as_nanos() as f64);
        }
        let t = mark();
        self.encode_passes(
            &mut enc,
            view,
            &order,
            &opaque_columns,
            &model_columns,
            &contact_columns,
            &mut stats,
            any_model_visible,
            any_transparent_visible,
        );
        if let Some(g) = &self.gpu_timer {
            g.cpu_stage("cpu: encode passes", t.elapsed().as_nanos() as f64);
        }
        self.terrain.draw_order = order;
        self.terrain.opaque_column_order = opaque_columns;
        self.terrain.model_column_order = model_columns;
        self.terrain.contact_column_order = contact_columns;
        if let Some(t) = &self.gpu_timer {
            t.finish_frame(&mut enc);
        }
        let t = mark();
        let cb = enc.finish();
        if let Some(g) = &self.gpu_timer {
            g.cpu_stage("cpu: encoder finish", t.elapsed().as_nanos() as f64);
        }
        let t = mark();
        self.queue.submit(std::iter::once(cb));
        if let Some(g) = &self.gpu_timer {
            g.cpu_stage("cpu: queue submit", t.elapsed().as_nanos() as f64);
        }
        if let Some(t) = &self.gpu_timer {
            t.after_submit(&self.device);
        }
        self.last_stats = stats;
    }
}
