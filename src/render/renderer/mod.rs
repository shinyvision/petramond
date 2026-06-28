use crate::camera::{Camera, Frustum};
use crate::chunk::{ChunkPos, CHUNK_SY};
use crate::mathh::SelectionShape;
use crate::world::{TerrainMeshUploadSource, TerrainVisibilitySource};

use std::collections::HashMap;
use wgpu::util::DeviceExt;

mod construct;
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

use super::block_model::BillboardBasis;
use super::break_overlay::build_break_overlay;
use super::chest_model::build_chests;
use super::crosshair::crosshair_vertices;
use super::door_model::build_doors;
use super::hand::build_hand_lit;
use super::hand_animator::HeldItemAnimator;
use super::item_entity::build_item_entities;
use super::item_model::ItemVertex;
use super::mob_model::build_mob_instances;
use super::particles::build_particles_split;
use super::pipeline::create_pipeline_resources;
use super::resources::{
    create_atlas, create_depth, create_gui_panel, create_model_texture, upload_mesh, GpuMesh,
};
use super::section_cull::{section_draw_ranges, SectionVisibilityCache};
use super::selection::outline_vertices;
use super::ui::{build_ui, UiBuild, UiVertex};
use super::uniforms::{Uniforms, FOG_END, FOG_START, UNDERWATER_FOG_END, UNDERWATER_FOG_START};
use super::{
    BreakOverlayView, ChestInstance, DoorInstance, HeldItemFrame, HeldItemView, ItemEntityInstance,
    MobRenderInstance, ParticleInstance,
};
use crate::bbmodel::Model;
use crate::gui::{GuiKind, OverlayTag, UiSnapshot};

/// Key into the renderer's data-driven GUI texture map: every baked PNG a GUI can
/// draw — its panel, its hover/selection highlight, and each dynamic overlay — has
/// its own bind group, looked up by (kind, role) so the UI pass binds the right
/// texture per quad without any per-screen branching.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum GuiTexId {
    Panel(GuiKind),
    Hover(GuiKind),
    Overlay(GuiKind, OverlayTag),
}

#[derive(Copy, Clone, Debug, Default)]
pub(in crate::render) struct RenderStats {
    pub opaque_draws: u32,
    pub transparent_draws: u32,
    pub opaque_indices: u64,
    pub transparent_indices: u64,
    pub section_culled_indices: u64,
}

/// Per-species GPU resources for the mob pipeline, built once at renderer init by
/// iterating [`crate::mob::MOB_DEFS`] (so the renderer never names a species). Borrows
/// the species' precached [`Model`] + its render scale, the species' own texture/sampler + group(1)
/// bind, its dynamic draw buffers, and reused per-frame scratch (the visible subset
/// + the baked `ItemVertex` geometry). The `Vec<MobGpu>` is in `Mob as usize` order.
struct MobGpu {
    model: &'static Model,
    scale: f32,
    // Kept alive alongside the bind group that references them.
    #[allow(dead_code)]
    texture: wgpu::Texture,
    #[allow(dead_code)]
    view: wgpu::TextureView,
    #[allow(dead_code)]
    sampler: wgpu::Sampler,
    bind: wgpu::BindGroup,
    draw: DynamicDraw,
    /// Frustum-visible subset of this species' instances this frame.
    visible: Vec<MobRenderInstance>,
    /// Reused CPU staging for this species' baked geometry.
    verts: Vec<ItemVertex>,
    indices: Vec<u32>,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    // Held to keep the GPU atlas texture/view/sampler alive for `atlas_bind`'s lifetime;
    // dropping them could free resources the draw-time bind group still references.
    #[allow(dead_code)]
    atlas_texture: wgpu::Texture,
    #[allow(dead_code)]
    atlas_view: wgpu::TextureView,
    #[allow(dead_code)]
    atlas_sampler: wgpu::Sampler,
    sky_pipe: wgpu::RenderPipeline,
    sky_bind: wgpu::BindGroup,
    opaque_pipe: wgpu::RenderPipeline,
    transparent_pipe: wgpu::RenderPipeline,
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
    /// Currently-targeted outline shape, or None when nothing is targeted.
    selection: Option<SelectionShape>,
    /// The target whose geometry currently sits in `outline_vbuf`.
    selection_drawn: Option<SelectionShape>,
    uniform_buf: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    atlas_bind: wgpu::BindGroup,
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
    /// order). Built once from `mob::MOB_DEFS`; each frame the visible mobs are
    /// grouped here by species, baked, and drawn in the mob pass.
    mob_gpu: Vec<MobGpu>,
    /// bbmodel-block ("model") render resources: the mob pipeline reused for the model
    /// pass plus the combined model atlas bound at group(1). The geometry itself lives
    /// in each chunk's `GpuMesh::model_*` (baked by the mesher), so there's no per-frame
    /// model bake — the model pass just draws the chunks' model streams.
    model_pipe: wgpu::RenderPipeline,
    #[allow(dead_code)]
    model_atlas_texture: wgpu::Texture,
    #[allow(dead_code)]
    model_atlas_view: wgpu::TextureView,
    #[allow(dead_code)]
    model_atlas_sampler: wgpu::Sampler,
    model_atlas_bind: wgpu::BindGroup,
    /// Dropped bbmodel item-entities (world-space ItemVertex, model atlas), drawn by the
    /// model pipeline in the model pass — the explicit-UV counterpart of `item_entity_draw`.
    item_model_entity_draw: DynamicDraw,
    item_model_entity_verts: Vec<super::item_model::ItemVertex>,
    item_model_entity_indices: Vec<u32>,
    /// Particle billboard draw: the particle pipeline + a per-frame vbuf and a
    /// STATIC quad ibuf, as one [`DynamicVertexDraw`].
    particle_draw: DynamicVertexDraw,
    depth: wgpu::TextureView,
    chunk_meshes: HashMap<ChunkPos, GpuMesh>,
    /// Reusable per-frame draw order: `(dist_sq, ChunkPos)` for the visible chunks,
    /// sorted near→far. Cleared + refilled each `render` (capacity retained) so the
    /// frame never heap-allocates the sort list; the passes look meshes up by key.
    draw_order: Vec<(f32, ChunkPos)>,
    /// Camera frustum for viewspace culling, refreshed each frame in
    /// `update_uniforms`; chunk meshes outside it are skipped in `render`.
    frustum: Frustum,
    /// Camera world position, refreshed in `update_uniforms`; used to sort
    /// chunk draws front-to-back (opaque) / back-to-front (transparent).
    cam_pos: glam::Vec3,
    section_visibility: SectionVisibilityCache,
    /// Background clear colour, kept in sync with the fog colour each frame (sky/
    /// biome fog above water, deep blue when submerged) so the horizon matches the
    /// fog the terrain fades into.
    clear_color: [f32; 3],
    last_stats: RenderStats,
    // --- Per-frame view state set by the App via setters, drawn in `render`. ---
    /// Block-break overlay to draw this frame, or `None`.
    break_overlay: Option<BreakOverlayView>,
    /// First-person held item / hand state (defaults to the bare hand).
    held_item: HeldItemView,
    held_item_anim: HeldItemAnimator,
    held_item_skylight: u8,
    held_item_warm: u8,
    /// Dropped item-entities to draw in the world this frame.
    item_entities: Vec<ItemEntityInstance>,
    /// Block-atlas particle cubes to draw this frame.
    particles: Vec<ParticleInstance>,
    /// Model-atlas particle cubes (bbmodel-block flecks) to draw this frame — baked into
    /// the SAME particle vbuf after the block cubes, then drawn with the model atlas bound.
    model_particles: Vec<ParticleInstance>,
    /// Vertex count of the BLOCK-atlas portion of `particle_draw` this frame (the split
    /// point: `[0..this)` draws with the block atlas, the rest with the model atlas).
    particle_block_vertex_count: u32,
    /// Snapshot of the UI/inventory to draw (owned, no borrow held).
    ui: UiSnapshot,
    /// Camera right/up basis for world-space billboards (item sprites + particles),
    /// refreshed in `update_uniforms` from the inverse view rotation.
    billboard_basis: BillboardBasis,
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
    /// UI pipeline (2D HUD / inventory). Every UI quad is drawn with it; group(0)
    /// binds whichever baked texture (or the icon atlas) the quad samples.
    ui_pipe: wgpu::RenderPipeline,
    /// Every baked GUI texture (panel / hover / overlay) as its own bind group,
    /// keyed by [`GuiTexId`]. Loaded from disk at init; the UI pass looks each up
    /// by the open kind. See `crate::gui`.
    gui_textures: std::collections::HashMap<GuiTexId, wgpu::BindGroup>,
    /// Solid-color quads (the menu dim backdrop + all stack-count digits) packed
    /// into one buffer: dim `[0, dim)`, then normal counts, then drag counts. Drawn
    /// with the icon-atlas bind (the solid sentinel skips the sampler anyway).
    ui_solid_vbuf: wgpu::Buffer,
    ui_dim_vertex_count: u32,
    ui_count_vertex_count: u32,
    ui_drag_count_vertex_count: u32,
    /// The baked panel-PNG quad for the open GUI + its vertex count.
    ui_panel_vbuf: wgpu::Buffer,
    ui_panel_vertex_count: u32,
    /// Dynamic overlay quads (furnace gauges) concatenated; `ui_build.overlay_spans`
    /// says how to slice + bind them per [`OverlayTag`].
    ui_overlay_vbuf: wgpu::Buffer,
    ui_overlay_vertex_count: u32,
    /// The hover / selection highlight quad + its vertex count.
    ui_hover_vbuf: wgpu::Buffer,
    ui_hover_vertex_count: u32,
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

impl Renderer {
    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.refresh_overlay_buffers();
        self.prepare_held_item();
        self.build_ui_frame();
        self.bake_world_instances();

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        // Reusable draw order taken out so `plan_draw_order` can fill it while
        // `self` is read; restored after encoding (capacity retained next frame).
        let mut order = std::mem::take(&mut self.draw_order);
        let (mut stats, any_model_visible) = self.plan_draw_order(&mut order);
        self.encode_passes(&mut enc, &view, &order, &mut stats, any_model_visible);
        self.draw_order = order;
        self.queue.submit(std::iter::once(enc.finish()));
        self.last_stats = stats;
        frame.present();
    }
}
