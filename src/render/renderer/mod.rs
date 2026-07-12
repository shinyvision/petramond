use crate::camera::{Camera, Frustum};
use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::SelectionShape;
use crate::world::TerrainRenderHandoff;

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
mod passes;
mod ui_frame;

#[cfg(test)]
pub(in crate::render) use construct::instance_descriptor;
pub(crate) use construct::new_renderer_from_target;
use dynamic_draw::{DynamicDraw, DynamicVertexDraw};
use icon_atlas::IconAtlas;
use lod::far_leaf_lod_active;

use super::break_overlay::build_break_overlays;
use super::chest_model::build_chests;
use super::crosshair::crosshair_vertices;
use super::door_model::build_doors;
use super::hand::build_hand_lit;
use super::hand_animator::HeldItemAnimator;
use super::item_entity::build_item_entities;
use super::item_model::ItemVertex;
use super::mob_model::build_mob_instances;
use super::particles::{build_particles_split, build_transparent_emitter_particles};
use super::pipeline::create_pipeline_resources;
use super::resources::{
    create_atlas, create_atlas_array, create_depth, create_gui_panel, create_model_texture,
    create_scene_color, upload_column_mesh, ColumnUploadScratch, GpuColumnMesh, GpuSectionMesh,
};
use super::selection::outline_vertices;
use super::ui::{build_ui, UiBuild, UiVertex};
use super::uniforms::{Uniforms, UNDERWATER_FOG_END, UNDERWATER_FOG_START};
use super::{
    BreakOverlayView, ChestInstance, DoorInstance, HeldItemFrame, HeldItemView, ItemEntityInstance,
    MobRenderInstance, ParticleEmitterInstance, ParticleInstance, PlayerRenderInstance,
    RemotePlayerRender, SolidParticleInstance, UiFrame,
};
use crate::bbmodel::Model;
use crate::gui::{UiSnapshot, UiViewport};

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

#[inline]
fn aabb_distance_sq(p: glam::Vec3, min: glam::Vec3, max: glam::Vec3) -> f32 {
    let dx = if p.x < min.x {
        min.x - p.x
    } else if p.x > max.x {
        p.x - max.x
    } else {
        0.0
    };
    let dy = if p.y < min.y {
        min.y - p.y
    } else if p.y > max.y {
        p.y - max.y
    } else {
        0.0
    };
    let dz = if p.z < min.z {
        min.z - p.z
    } else if p.z > max.z {
        p.z - max.z
    } else {
        0.0
    };
    dx * dx + dy * dy + dz * dz
}

#[derive(Copy, Clone, Debug, Default)]
pub(in crate::render) struct RenderStats {
    pub opaque_draws: u32,
    pub transparent_draws: u32,
    pub opaque_indices: u64,
    pub transparent_indices: u64,
}

#[derive(Copy, Clone)]
pub(in crate::render) struct VisibleSection {
    dist_sq: f32,
    column_pos: ChunkPos,
    opaque_batched: bool,
    model_batched: bool,
    use_far_leaf_lod: bool,
    opaque_index_start: u32,
    opaque_idx_count: u32,
    far_opaque_index_start: u32,
    far_opaque_idx_count: u32,
    transparent_index_start: u32,
    transparent_idx_count: u32,
    model_index_start: u32,
    model_idx_count: u32,
}

/// Per-species GPU resources for the mob pipeline, built once at renderer init by
/// iterating [`crate::mob::defs()`] (so the renderer never names a species). Borrows
/// the species' precached [`Model`] + its render scale, the species' own texture/sampler + group(1)
/// bind, its dynamic draw buffers, and reused per-frame scratch (the visible subset
/// + the baked `ItemVertex` geometry). The `Vec<MobGpu>` is in `Mob as usize` order.
struct MobGpu {
    model: &'static Model,
    scale: f32,
    bind: wgpu::BindGroup,
    draw: DynamicDraw,
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

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// The swapchain was rebuilt in response to a suboptimal acquire and came
    /// back STILL suboptimal — stop retrying (some drivers, e.g. NVIDIA on
    /// Wayland, report suboptimal permanently; reconfiguring every frame would
    /// recreate the swapchain at frame rate). Cleared by a good acquire or a
    /// real resize, so genuine size/scale mismatches always get one rebuild.
    suboptimal_retried: bool,
    sky_pipe: wgpu::RenderPipeline,
    sky_bind: wgpu::BindGroup,
    sky_texture_bind: wgpu::BindGroup,
    sky_shader_param_keys: Vec<String>,
    sky_light_param_key: Option<String>,
    underwater: bool,
    /// Above-water fog band, derived from the streaming render distance
    /// (`uniforms::fog_range`) via [`Renderer::set_render_distance`] so the fade
    /// always terminates at the loaded-world edge. The end (plus
    /// `TERRAIN_FOG_CULL_PAD`) is also the terrain draw-cull distance.
    fog_start: f32,
    fog_end: f32,
    /// Sim-owned skylight scale (1.0 = identity), mirrored to the CPU lighting
    /// path (`render::lighting::light_rgb`) for mobs/items/particles.
    sky_scale: f32,
    /// Sim-owned sky light colour (white = identity), the CPU mirror of the
    /// `sky_color` uniform lane — applied to the SKY term only.
    sky_color: [f32; 3],
    opaque_pipe: wgpu::RenderPipeline,
    transparent_pipe: wgpu::RenderPipeline,
    /// Offscreen scene-colour target the world passes render into; the grade
    /// pass reads it and writes the swapchain. Recreated with `depth` on resize.
    scene_color: wgpu::TextureView,
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
    uniform_buf: wgpu::Buffer,
    shader_params_buf: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    atlas_bind: wgpu::BindGroup,
    /// Terrain tile-ARRAY bind (group 1 for the opaque/transparent block pipelines),
    /// parallel to `atlas_bind`; the block terrain draws bind this, everything else the 2D atlas.
    atlas_array_bind: wgpu::BindGroup,
    /// Depth-enabled model3d variant for the first-person held block in the hand
    /// pass (same shader; the hand pass clears depth so the held block self-sorts).
    /// (The depthless `model3d_pipe` is now used only to bake the icon atlas at init,
    /// so it isn't stored here.)
    model3d_hand_pipe: wgpu::RenderPipeline,
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
    hand_index_count: u32,
    /// Vertex count of the hand geometry (icons are appended after it in the
    /// shared model3d vbuf, so their `base_vertex` starts here).
    hand_vertex_count: u32,
    /// Reusable CPU staging for the per-frame hand geometry (cleared + refilled by
    /// `build_hand`, capacity retained — no per-frame allocation).
    hand_verts: Vec<crate::mesh::Vertex>,
    hand_indices: Vec<u32>,
    /// Break-overlay (destroy crack): its own pipeline + dynamic vbuf/ibuf + the
    /// index count baked this frame (0 = no overlay), as one [`DynamicDraw`].
    break_draw: DynamicDraw,
    /// Item-entity dynamic draw (drawn by the EXISTING opaque pipeline — a cloned
    /// handle — over its OWN fixed-size buffers, sized separately from chests).
    item_entity_draw: DynamicDraw,
    /// Chest model dynamic draw (opaque pipeline, like item entities; its caps are
    /// separate so a wall of chests can't make dropped items vanish).
    chest_draw: DynamicDraw,
    /// Door model dynamic draw (opaque pipeline like chests; separate caps so a wall of
    /// doors can't make chests vanish).
    door_draw: DynamicDraw,
    /// Per-species mob render resources, indexed by `Mob as usize` (registry id
    /// order). Built once from `mob::defs()`; each frame the visible mobs are
    /// grouped here by species, baked, and drawn in the mob pass.
    mob_gpu: Vec<MobGpu>,
    /// Player-body resources (local third-person + remote players, one
    /// combined stream drawn in the mob pass).
    player_gpu: PlayerGpu,
    /// The LOCAL third-person body to draw this frame (`None` in first
    /// person). Its held item reads the renderer's own first-person
    /// `held_item` view — unchanged solo behavior.
    player_view: Option<PlayerRenderInstance>,
    /// The remote players' bodies + held-item views for this frame.
    remote_players: Vec<RemotePlayerRender>,
    /// Frustum-visible bodies this frame (local first, then remotes), each
    /// paired with the held-item view that animates its hand.
    player_visible: Vec<(PlayerRenderInstance, HeldItemView)>,
    /// Per-body staging for one `build_player_body` bake (the builder clears
    /// its buffers), appended into `player_gpu`'s combined stream.
    player_body_verts: Vec<super::item_model::ItemVertex>,
    player_body_indices: Vec<u32>,
    /// Held EXTRUDED-SPRITE items across all bodies (explicit-UV stream, 2D
    /// atlas), attached to each posed right hand. Rides the mob-layout
    /// pipeline.
    player_item_draw: DynamicDraw,
    player_item_verts: Vec<super::item_model::ItemVertex>,
    player_item_indices: Vec<u32>,
    /// Per-item staging for one extruded-sprite build (the builder clears).
    player_sprite_verts: Vec<super::item_model::ItemVertex>,
    /// Held BBMODEL items across all bodies (explicit-UV stream, MODEL
    /// atlas) — split from the sprite stream so mixed hands draw with the
    /// right texture in one pass each.
    player_model_item_draw: DynamicDraw,
    player_model_item_verts: Vec<super::item_model::ItemVertex>,
    player_model_item_indices: Vec<u32>,
    /// Held BLOCK mini-cubes across all bodies (packed block vertices, opaque
    /// pipeline + terrain atlas array), CPU-transformed to each hand like
    /// dropped item cubes.
    player_block_item_draw: DynamicDraw,
    /// bbmodel-block ("model") render resources: the mob pipeline reused for the model
    /// pass plus the combined model atlas bound at group(1). The geometry itself lives
    /// in packed terrain columns as per-section model ranges, so there's no per-frame
    /// model bake — the model pass just draws the visible sections' model streams.
    model_pipe: wgpu::RenderPipeline,
    /// Pipeline for the chunk `ModelVertex` stream (day/night-aware lighting);
    /// `model_pipe` (mob layout) keeps drawing dropped bbmodel item entities.
    world_model_pipe: wgpu::RenderPipeline,
    model_atlas_bind: wgpu::BindGroup,
    /// Dropped bbmodel item-entities (world-space ItemVertex, model atlas), drawn by the
    /// model pipeline in the model pass — the explicit-UV counterpart of `item_entity_draw`.
    item_model_entity_draw: DynamicDraw,
    item_model_entity_verts: Vec<super::item_model::ItemVertex>,
    item_model_entity_indices: Vec<u32>,
    /// Dropped SPRITE item-entities extruded into pixel-perfect 3D slabs
    /// (world-space ItemVertex, 2D block atlas — the wall UVs address single
    /// texels), drawn in the item-entity pass on the mob-layout pipeline.
    item_sprite_entity_draw: DynamicDraw,
    item_sprite_entity_verts: Vec<super::item_model::ItemVertex>,
    item_sprite_entity_indices: Vec<u32>,
    /// Per-instance staging for one extruded-sprite build (the builder clears).
    item_sprite_scratch: Vec<super::item_model::ItemVertex>,
    /// Particle billboard draw: the particle pipeline + a per-frame vbuf and a
    /// STATIC quad ibuf, as one [`DynamicVertexDraw`].
    particle_draw: DynamicVertexDraw,
    /// Translucent block-emitter particles: same cube vertex format as mining dust,
    /// but a separate alpha-blended pipeline/vbuf so cutout dust remains unchanged.
    emitter_particle_draw: DynamicVertexDraw,
    depth: wgpu::TextureView,
    terrain_columns: HashMap<ChunkPos, GpuColumnMesh>,
    /// Persistent upload work. World dirtiness is level-triggered, so the set
    /// deduplicates columns while the heap preserves their first useful priority.
    terrain_upload_pending: HashMap<ChunkPos, PendingTerrainUpload>,
    terrain_upload_heap: BinaryHeap<Reverse<(u8, u32, i32, i32, u64)>>,
    terrain_upload_frame: u64,
    /// Reusable CPU staging for packing section meshes into a GPU column upload.
    terrain_upload_scratch: ColumnUploadScratch,
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
    terrain_gpu_revision: u64,
    terrain_planned_gpu_revision: u64,
    terrain_view_key: TerrainViewKey,
    terrain_planned_view_key: Option<TerrainViewKey>,
    terrain_plan_any_model: bool,
    terrain_plan_any_transparent: bool,
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
    /// Sections currently drawing the far leaf mesh. Stored only for active far-LOD
    /// sections so the transition has hysteresis instead of flipping at one threshold.
    far_leaf_lod_state: HashMap<SectionPos, bool>,
    /// Background clear colour, kept in sync with the fog colour each frame (sky/
    /// biome fog above water, deep blue when submerged) so the horizon matches the
    /// fog the terrain fades into.
    clear_color: [f32; 3],
    last_stats: RenderStats,
    // --- Per-frame view state handed off by the App, drawn in `render`. ---
    /// Block-break overlays to draw this frame (own + capped remotes; empty =
    /// none).
    break_overlays: Vec<BreakOverlayView>,
    /// First-person held item / hand state (defaults to the bare hand).
    held_item: HeldItemView,
    hand_visible: bool,
    /// Screen-space (NDC) offset applied to the whole hand/held-item draw this
    /// frame — the hurt-shake jitter. Zero when calm.
    hand_shake: [f32; 2],
    held_item_anim: HeldItemAnimator,
    held_item_skylight: u8,
    held_item_blocklight: u8,
    held_item_warm: u8,
    /// Dropped item-entities to draw in the world this frame.
    item_entities: Vec<ItemEntityInstance>,
    /// Block-atlas particle cubes to draw this frame.
    particles: Vec<ParticleInstance>,
    /// Model-atlas particle cubes (bbmodel-block flecks) to draw this frame — baked into
    /// the SAME particle vbuf after the block cubes, then drawn with the model atlas bound.
    model_particles: Vec<ParticleInstance>,
    /// Solid-color simulated particles (emitter-burst droplets) joining the
    /// emitter cubes' alpha-blended bake.
    solid_particles: Vec<SolidParticleInstance>,
    /// Loaded block-row particle emitters to synthesize into translucent cube particles.
    particle_emitters: Vec<ParticleEmitterInstance>,
    /// Frustum/fog-visible subset of `particle_emitters`.
    particle_emitter_visible: Vec<ParticleEmitterInstance>,
    /// See [`Renderer::set_particle_density`].
    particle_density: f32,
    /// Vertex count of the BLOCK-atlas portion of `particle_draw` this frame (the split
    /// point: `[0..this)` draws with the block atlas, the rest with the model atlas).
    particle_block_vertex_count: u32,
    /// Surface generation used to reject a complete UI frame solved before a
    /// resize, plus the viewport of the most recently prepared coherent UI.
    viewport_generation: u64,
    prepared_ui_viewport: UiViewport,
    /// Reusable CPU staging for baked item-entity geometry (cleared + refilled per
    /// frame, capacity retained).
    item_entity_verts: Vec<crate::mesh::Vertex>,
    item_entity_indices: Vec<u32>,
    /// Reusable scratch for the frustum-visible subset of `item_entities`.
    item_entity_visible: Vec<ItemEntityInstance>,
    /// Placed chests to draw in the world this frame.
    chests: Vec<ChestInstance>,
    /// Reusable scratch for the frustum-visible subset of `chests`.
    chest_visible: Vec<ChestInstance>,
    /// Placed doors to draw in the world this frame.
    doors: Vec<DoorInstance>,
    /// Reusable scratch for the frustum-visible subset of `doors`.
    door_visible: Vec<DoorInstance>,
    /// Mobs to draw in the world this frame (the scene adapter fills this by
    /// interpolating the sim's live mob instances). The per-species visible subset +
    /// baked geometry live in `mob_gpu`.
    mobs: Vec<MobRenderInstance>,
    /// Reusable CPU staging for baked particle vertices.
    particle_verts: Vec<super::particles::ParticleVertex>,
    /// Reusable CPU staging for translucent emitter-particle vertices.
    emitter_particle_verts: Vec<super::particles::ParticleVertex>,
    /// Reusable generated translucent particle rows, sorted far-to-near before vertex bake.
    emitter_particle_scratch: Vec<super::particles::TransparentParticleCube>,
    /// UI pipeline (2D HUD / inventory). Every UI quad is drawn with it; group(0)
    /// binds whichever baked texture (or the icon atlas) the quad samples.
    ui_pipe: wgpu::RenderPipeline,
    /// Texture+sampler bind layout used by every UI texture (doc-UI images,
    /// the heart atlas).
    ui_texture_bgl: wgpu::BindGroupLayout,
    /// GUI-document draw path (petramond-ui DrawList upload + batches): every
    /// screen's chrome. See `doc_ui`.
    doc_ui: doc_ui::DocUi,
    /// Client-WASM images drawn directly in physical screen pixels (HUD
    /// overlays and the active modal canvas), outside document layout.
    client_overlays: client_overlay::ClientOverlays,
    /// Solid-color quads (all stack-count digits) packed into one buffer:
    /// normal counts `[0, counts)`, then drag counts. Drawn with the
    /// icon-atlas bind (the solid sentinel skips the sampler anyway).
    ui_solid_vbuf: wgpu::Buffer,
    ui_count_vertex_count: u32,
    ui_drag_count_vertex_count: u32,
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
    /// Vertex count of the cursor-held icon quads appended after normal icons.
    drag_icon_quad_vertex_count: u32,
    /// Reusable CPU staging for the per-frame UI geometry (all quad buffers +
    /// overlay spans + icon-quad list), cleared + refilled each frame.
    ui_build: UiBuild,
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
        self.fog_start = start;
        self.fog_end = end;
    }

    /// Terrain draw-cull distance: nothing beyond this is fully un-fogged.
    pub(in crate::render) fn terrain_cull_dist(&self) -> f32 {
        self.fog_end + TERRAIN_FOG_CULL_PAD
    }

    /// Emitter-derived particle density from the particles graphics option
    /// (`0` = off, `0.5` = reduced, `1` = full). Scales each looping emitter's
    /// active-particle count; zero skips emitter baking entirely.
    pub fn set_particle_density(&mut self, density: f32) {
        self.particle_density = density.clamp(0.0, 1.0);
    }

    /// Set the internal world-resolution scale (clamped `0.5..=1.0`) and rebuild
    /// the offscreen targets. The grade pass upscales to the swapchain.
    pub fn set_render_scale(&mut self, scale: f32) {
        let scale = scale.clamp(0.5, 1.0);
        if (scale - self.render_scale).abs() < f32::EPSILON {
            return;
        }
        self.render_scale = scale;
        self.recreate_scene_targets();
    }

    /// Toggle the colour-grade pass. Off (at native scale) skips the offscreen
    /// scene round-trip entirely — the world renders straight to the swapchain.
    pub fn set_grade_enabled(&mut self, on: bool) {
        self.grade_enabled = on;
    }

    /// World passes bypass the offscreen target only when nothing needs it:
    /// grade off AND native scale (upscaling needs the small target + grade).
    pub(in crate::render) fn direct_to_swapchain(&self) -> bool {
        !self.grade_enabled && self.render_scale >= 1.0
    }

    /// The offscreen scene/depth dimensions under `render_scale`.
    pub(in crate::render) fn scene_dims(&self) -> (u32, u32) {
        let scale = self.render_scale;
        (
            ((self.config.width as f32 * scale).round() as u32).max(1),
            ((self.config.height as f32 * scale).round() as u32).max(1),
        )
    }

    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            // A suboptimal frame still presents (with a per-present driver
            // warning), but the swapchain no longer matches the surface —
            // rebuild it once and draw from the fresh one next frame. The
            // frame must drop BEFORE the reconfigure (a live SurfaceTexture
            // across a swapchain rebuild panics).
            Ok(t) if t.suboptimal && !self.suboptimal_retried => {
                self.suboptimal_retried = true;
                drop(t);
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Ok(t) => {
                self.suboptimal_retried = t.suboptimal;
                t
            }
            // Stale/lost swapchain (a resize or compositor change the events
            // haven't delivered yet): reconfigure at the current size and let
            // the next frame draw.
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(_) => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.refresh_overlay_buffers();
        self.prepare_held_item();
        self.bake_world_instances();

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        // Reusable draw orders taken out so `plan_draw_order` can fill them while
        // `self` is read; restored after encoding (capacity retained next frame).
        let mut order = std::mem::take(&mut self.draw_order);
        let mut opaque_columns = std::mem::take(&mut self.opaque_column_order);
        let mut model_columns = std::mem::take(&mut self.model_column_order);
        let (mut stats, any_model_visible, any_transparent_visible) =
            self.plan_draw_order(&mut order, &mut opaque_columns, &mut model_columns);
        self.encode_passes(
            &mut enc,
            &view,
            &order,
            &opaque_columns,
            &model_columns,
            &mut stats,
            any_model_visible,
            any_transparent_visible,
        );
        self.draw_order = order;
        self.opaque_column_order = opaque_columns;
        self.model_column_order = model_columns;
        self.queue.submit(std::iter::once(enc.finish()));
        self.last_stats = stats;
        frame.present();
    }
}
