use crate::camera::{Camera, Frustum};
use crate::chunk::{ChunkPos, CHUNK_SY};
use crate::mathh::SelectionShape;
use crate::world::World;

use std::collections::HashMap;
use wgpu::util::DeviceExt;

mod dynamic_draw;
mod icon_atlas;
mod lod;

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
use super::gui_def::{GuiKind, OverlayTag};
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
    MobRenderInstance, ParticleInstance, UiFrame,
};
use crate::bbmodel::Model;
use crate::inventory::TOTAL_SLOTS;
use crate::item::ItemType;

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
pub struct RenderStats {
    pub frustum_chunks: u32,
    pub drawn_chunks: u32,
    pub visible_sections: u32,
    pub opaque_draws: u32,
    pub transparent_draws: u32,
    pub opaque_indices: u64,
    pub transparent_indices: u64,
    pub section_culled_indices: u64,
    pub section_culling_active: bool,
}

/// An owned, render-side snapshot of the bits of [`UiFrame`] the renderer needs
/// to draw the hotbar / open inventory. Captured each frame in
/// [`Renderer::set_ui`] so the renderer never holds a borrow of the game's
/// `Inventory` across frames (render reads flat data — contract §rules).
#[derive(Clone, Debug)]
pub struct UiSnapshot {
    pub open: bool,
    /// Which baked GUI this frame draws — the open menu's kind, or `Hotbar` for the
    /// HUD. Selects the panel/hover/overlay textures and the slot layout.
    pub kind: GuiKind,
    pub screen: (u32, u32),
    pub cursor_px: (f32, f32),
    pub active: u8,
    /// One entry per inventory slot (`[0,9)` hotbar, `[9,36)` main grid):
    /// `(item, count)`, or `None` for an empty slot.
    pub slots: [Option<(ItemType, u8)>; TOTAL_SLOTS],
    /// The crafting input cells (only the first `panel.cols()²` are drawn).
    pub craft: [Option<(ItemType, u8)>; crate::crafting::MAX_CELLS],
    /// The crafting result preview, drawn in the result slot.
    pub result: Option<(ItemType, u8)>,
    /// The cursor-held stack (drag/drop), drawn at `cursor_px` when open.
    pub cursor: Option<(ItemType, u8)>,
    /// The open furnace's slots + progress gauges, or `None` when the open panel is
    /// not a furnace. When `Some`, the furnace panel is drawn instead of the grid.
    pub furnace: Option<super::FurnaceView>,
    /// The open chest's 27 storage slots, or `None`. When `Some`, the chest panel +
    /// storage grid are drawn instead of the crafting grid.
    pub chest: Option<super::ChestView>,
    /// The open furniture workbench's input + offered results, or `None`. When `Some`,
    /// the workbench panel is drawn with the result grid (greyed where not craftable).
    pub workbench: Option<super::WorkbenchView>,
}

impl Default for UiSnapshot {
    fn default() -> Self {
        UiSnapshot {
            open: false,
            kind: GuiKind::Hotbar,
            screen: (0, 0),
            cursor_px: (0.0, 0.0),
            active: 0,
            slots: [None; TOTAL_SLOTS],
            craft: [None; crate::crafting::MAX_CELLS],
            result: None,
            cursor: None,
            furnace: None,
            chest: None,
            workbench: None,
        }
    }
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
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub atlas_texture: wgpu::Texture,
    pub atlas_view: wgpu::TextureView,
    pub atlas_sampler: wgpu::Sampler,
    pub sky_pipe: wgpu::RenderPipeline,
    pub sky_bind: wgpu::BindGroup,
    pub opaque_pipe: wgpu::RenderPipeline,
    pub transparent_pipe: wgpu::RenderPipeline,
    /// Pipeline for the targeted-block wireframe (LineList, black, view_proj only).
    pub outline_pipe: wgpu::RenderPipeline,
    pub outline_bind: wgpu::BindGroup,
    /// Line vertices for the selection outline; rewritten only when the selected
    /// target changes (see `selection` / `selection_drawn`).
    pub outline_vbuf: wgpu::Buffer,
    pub outline_vertex_count: u32,
    pub crosshair_pipe: wgpu::RenderPipeline,
    pub crosshair_vbuf: wgpu::Buffer,
    pub crosshair_vertex_count: u32,
    crosshair_drawn_size: (u32, u32),
    /// Currently-targeted outline shape, or None when nothing is targeted.
    pub selection: Option<SelectionShape>,
    /// The target whose geometry currently sits in `outline_vbuf`.
    selection_drawn: Option<SelectionShape>,
    pub uniform_buf: wgpu::Buffer,
    pub uniform_bind: wgpu::BindGroup,
    pub atlas_bind: wgpu::BindGroup,
    /// Depth-enabled model3d variant for the first-person held block in the hand
    /// pass (same shader; the hand pass clears depth so the held block self-sorts).
    /// (The depthless `model3d_pipe` is now used only to bake the icon atlas at init,
    /// so it isn't stored here.)
    pub model3d_hand_pipe: wgpu::RenderPipeline,
    /// Dynamic-offset MVP uniform buffer (256-byte slots); slot 0 is the hand.
    pub model3d_mvp_buf: wgpu::Buffer,
    /// group(0) bind for model3d (MVP at binding 0 + uv_rects at binding 1).
    pub model3d_mvp_bind: wgpu::BindGroup,
    /// Reusable dynamic vertex/index buffers for model3d draws (rewritten in place).
    pub model3d_vbuf: wgpu::Buffer,
    pub model3d_ibuf: wgpu::Buffer,
    /// item3d pipeline (extruded first-person held item) + its group0 MVP bind
    /// (over the shared `model3d_mvp_buf`, slot 0) and reusable dynamic vbuf.
    pub item3d_pipe: wgpu::RenderPipeline,
    pub item3d_mvp_bind: wgpu::BindGroup,
    pub item3d_vbuf: wgpu::Buffer,
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
    pub depth: wgpu::TextureView,
    pub chunk_meshes: HashMap<ChunkPos, GpuMesh>,
    /// Reusable per-frame draw order: `(dist_sq, ChunkPos)` for the visible chunks,
    /// sorted near→far. Cleared + refilled each `render` (capacity retained) so the
    /// frame never heap-allocates the sort list; the passes look meshes up by key.
    draw_order: Vec<(f32, ChunkPos)>,
    /// Camera frustum for viewspace culling, refreshed each frame in
    /// `update_uniforms`; chunk meshes outside it are skipped in `render`.
    pub frustum: Frustum,
    /// Camera world position, refreshed in `update_uniforms`; used to sort
    /// chunk draws front-to-back (opaque) / back-to-front (transparent).
    pub cam_pos: glam::Vec3,
    section_visibility: SectionVisibilityCache,
    /// Background clear colour, kept in sync with the fog colour each frame (sky/
    /// biome fog above water, deep blue when submerged) so the horizon matches the
    /// fog the terrain fades into.
    pub clear_color: [f32; 3],
    pub last_stats: RenderStats,
    // --- Per-frame view state set by the App via setters, drawn in `render`. ---
    /// Block-break overlay to draw this frame, or `None`.
    pub break_overlay: Option<BreakOverlayView>,
    /// First-person held item / hand state (defaults to the bare hand).
    pub held_item: HeldItemView,
    held_item_anim: HeldItemAnimator,
    held_item_skylight: u8,
    held_item_warm: u8,
    /// Dropped item-entities to draw in the world this frame.
    pub item_entities: Vec<ItemEntityInstance>,
    /// Block-atlas particle cubes to draw this frame.
    pub particles: Vec<ParticleInstance>,
    /// Model-atlas particle cubes (bbmodel-block flecks) to draw this frame — baked into
    /// the SAME particle vbuf after the block cubes, then drawn with the model atlas bound.
    pub model_particles: Vec<ParticleInstance>,
    /// Vertex count of the BLOCK-atlas portion of `particle_draw` this frame (the split
    /// point: `[0..this)` draws with the block atlas, the rest with the model atlas).
    particle_block_vertex_count: u32,
    /// Snapshot of the UI/inventory to draw (owned, no borrow held).
    pub ui: UiSnapshot,
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
    pub chests: Vec<ChestInstance>,
    /// Reusable scratch for the frustum-visible subset of `chests`.
    chest_visible: Vec<ChestInstance>,
    /// Placed doors to draw in the world this frame.
    pub doors: Vec<DoorInstance>,
    /// Reusable scratch for the frustum-visible subset of `doors`.
    door_visible: Vec<DoorInstance>,
    /// Mobs to draw in the world this frame (the scene adapter fills this by
    /// interpolating the sim's live mob instances). The per-species visible subset +
    /// baked geometry live in `mob_gpu`.
    pub mobs: Vec<MobRenderInstance>,
    /// Reusable CPU staging for baked particle vertices.
    particle_verts: Vec<super::particles::ParticleVertex>,
    /// UI pipeline (2D HUD / inventory). Every UI quad is drawn with it; group(0)
    /// binds whichever baked texture (or the icon atlas) the quad samples.
    pub ui_pipe: wgpu::RenderPipeline,
    /// Every baked GUI texture (panel / hover / overlay) as its own bind group,
    /// keyed by [`GuiTexId`]. Loaded from disk at init; the UI pass looks each up
    /// by the open kind. See `gui_def`.
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

/// Begin one render pass with a single color attachment over `view` and an
/// optional depth attachment over `depth`. Collapses the near-identical
/// `begin_render_pass` boilerplate every pass used to spell out — only the parts
/// that actually vary are parameters: the debug `label`, the color load-op
/// (`Clear` for the sky, `Load` everywhere after), and `depth_load`:
/// - `Some(load_op)` → attach `depth` with that depth load-op (always store),
///   no stencil — the world / overlay / hand passes.
/// - `None` → no depth attachment — the sky, crosshair, and UI passes.
///
/// The store-ops, `depth_slice`, `resolve_target`, `timestamp_writes`, and
/// `occlusion_query_set` are the same for every pass, so they live here.
fn color_depth_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
    depth: &'a wgpu::TextureView,
    label: &str,
    color_load: wgpu::LoadOp<wgpu::Color>,
    depth_load: Option<wgpu::LoadOp<f32>>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: color_load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: depth_load.map(|load| wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    })
}

pub async fn new_renderer_from_target(
    target: impl Into<wgpu::SurfaceTarget<'static>>,
    width: u32,
    height: u32,
) -> Renderer {
    let instance = wgpu::Instance::new(&instance_descriptor());
    let surface = instance.create_surface(target).expect("create surface");
    new_renderer_inner(instance, surface, width, height).await
}

pub async fn new_renderer_with_instance(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Renderer {
    new_renderer_inner(instance, surface, width, height).await
}

/// Instance descriptor selecting native backends (Vulkan/Metal/DX12/GL).
///
/// Honors `WGPU_BACKEND` (`vulkan` | `gl`) to pin a single backend; unset = all.
/// This matters on a hybrid-GPU Wayland session: the discrete NVIDIA GPU's Vulkan
/// WSI can't present to a Wayland surface it isn't driving (it reports
/// `VK_KHR_wayland_surface` present = false), so wgpu's surface-compatible pick
/// falls back to the Intel iGPU. Its EGL/GLES path *can* present there, so
/// `WGPU_BACKEND=gl` (with the EGL vendor pointed at NVIDIA) renders on the dGPU.
pub fn instance_descriptor() -> wgpu::InstanceDescriptor {
    let mut desc = wgpu::InstanceDescriptor::default();
    if let Ok(name) = std::env::var("WGPU_BACKEND") {
        match name.trim().to_ascii_lowercase().as_str() {
            "vulkan" | "vk" => desc.backends = wgpu::Backends::VULKAN,
            "gl" | "gles" | "opengl" => desc.backends = wgpu::Backends::GL,
            _ => {}
        }
    }
    desc
}

pub async fn new_renderer(surface: wgpu::Surface<'static>, width: u32, height: u32) -> Renderer {
    // NOTE: surface must be created from a wgpu::Instance that is *not*
    // dropped before this call. We create a fresh instance here which means
    // the caller must have created the surface from this same runtime. In
    // practice, prefer `new_renderer_from_target` so the surface and adapter
    // share the same instance.
    let instance = wgpu::Instance::new(&instance_descriptor());
    new_renderer_inner(instance, surface, width, height).await
}

async fn new_renderer_inner(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Renderer {
    // Try a high-performance adapter first. If it fails entirely (no adapter
    // compatible with the surface), retry with force_fallback_adapter to accept
    // the software/lowest-tier adapter rather than panicking.
    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
    {
        Ok(a) => a,
        Err(_) => {
            eprintln!("wgpu: primary adapter unavailable; trying fallback");
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: true,
                })
                .await
                .expect("no compatible wgpu adapter available")
        }
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default().using_alignment(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .expect("device");

    let config = surface
        .get_default_config(&adapter, width, height)
        .expect("surface config");
    let format = config.format;
    let sample_count = 1u32;
    surface.configure(&device, &config);

    let (atlas_texture, atlas_view, atlas_sampler) = create_atlas(&device, &queue);
    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniforms"),
        contents: bytemuck::cast_slice(&[Uniforms {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            cam_pos: [0.0; 4],
            fog: [FOG_START, FOG_END, 0.0, 0.0],
            fog_color: [0.60, 0.82, 1.00, 1.0],
            inv_view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            water_anim: crate::atlas::water_anim_uniform(),
        }]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let pipelines = create_pipeline_resources(
        &device,
        format,
        sample_count,
        &uniform_buf,
        &atlas_view,
        &atlas_sampler,
    );
    let depth = create_depth(&device, width, height);

    // Item entities + chests draw through the EXISTING opaque pipeline; clone its
    // (Arc-backed) handle so each `DynamicDraw` issues a byte-identical draw while
    // the `opaque_pipe` field below still owns the original.
    let item_entity_pipe = pipelines.opaque_pipe.clone();
    let chest_pipe = pipelines.opaque_pipe.clone();
    let door_pipe = pipelines.opaque_pipe.clone();

    // Build per-species mob render resources by iterating the mob registry: load each
    // species' `.bbmodel` (geometry + walk animation + embedded texture), upload its
    // texture as a dedicated atlas, build its group(1) bind, and give it its own
    // dynamic-draw buffers over the shared mob pipeline. Adding a species is a row in
    // `mob::MOB_DEFS` — no renderer edit. A model parse failure degrades to an empty
    // model (that species just doesn't draw) rather than crashing the renderer.
    let mob_gpu: Vec<MobGpu> = crate::mob::ALL_MOBS
        .iter()
        .map(|&kind| {
            let d = crate::mob::def(kind);
            // Borrow this species' precached model (compiled once on startup, shared with
            // the simulation — see `crate::mob::model`). The renderer never reads a
            // `.bbmodel`: at runtime the `.llmob` + this in-memory `Model` are golden.
            let model = crate::mob::model(kind);
            let (texture, view, sampler) = create_model_texture(
                &device,
                &queue,
                &model.texture_rgba,
                model.tex_w,
                model.tex_h,
            );
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mob atlas bg"),
                layout: &pipelines.atlas_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mob vbuf"),
                size: super::pipeline::MAX_MOB_VERTICES * std::mem::size_of::<ItemVertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mob ibuf"),
                size: super::pipeline::MAX_MOB_INDICES * 4,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            MobGpu {
                model,
                scale: d.scale,
                texture,
                view,
                sampler,
                bind,
                draw: DynamicDraw::new(
                    pipelines.mob_pipe.clone(),
                    vbuf,
                    ibuf,
                    super::pipeline::MAX_MOB_VERTICES,
                    super::pipeline::MAX_MOB_INDICES,
                ),
                visible: Vec::new(),
                verts: Vec::new(),
                indices: Vec::new(),
            }
        })
        .collect();

    // bbmodel-block ("model") render resources: the combined model atlas (all kinds'
    // textures packed into one sheet — see `block_model::atlas`) uploaded as its own GPU
    // texture, bound at group(1) over the same atlas layout the mob pass uses, and the
    // mob pipeline reused for the model pass (the chunk's `ModelVertex` stream shares the
    // mob `ItemVertex` layout). The mesher bakes geometry into each chunk's model stream;
    // this pass just draws it with full-block lighting already baked in.
    let model_atlas = crate::block_model::atlas();
    let (matlas_rgba, matlas_w, matlas_h) = model_atlas.texture();
    let (model_atlas_texture, model_atlas_view, model_atlas_sampler) =
        create_model_texture(&device, &queue, matlas_rgba, matlas_w, matlas_h);
    let model_atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model atlas bg"),
        layout: &pipelines.atlas_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&model_atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&model_atlas_sampler),
            },
        ],
    });
    let model_pipe = pipelines.mob_pipe.clone();
    // Dropped bbmodel item-entities ride the model pipeline (world-space ItemVertex,
    // model atlas) in their OWN buffers, sized like the packed item-entity buffers.
    let item_model_entity_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item model entity vbuf"),
        size: super::pipeline::MAX_MOB_VERTICES * std::mem::size_of::<ItemVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let item_model_entity_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item model entity ibuf"),
        size: super::pipeline::MAX_MOB_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Bake every item's inventory icon into the icon atlas ONCE, here at init: the
    // cube/sprite icons through the depthless `model3d_pipe` and the bbmodel-block
    // icons through the depth-tested `model_icon_pipe` (these two pipelines are used
    // only by this bake now — see `icon_atlas`). The atlas color format MUST match
    // the surface (sRGB) so sampling/store cancel like the gui atlas (no double
    // gamma). The per-slot UI pass then draws a textured quad sampling this.
    let icon_atlas = icon_atlas::bake(
        &device,
        &queue,
        format,
        &pipelines.atlas_bgl,
        &pipelines.atlas_bind,
        &model_atlas_bind,
        &pipelines.model3d_pipe,
        &pipelines.model_icon_pipe,
        &pipelines.model3d_mvp_bgl,
        &pipelines.uv_rects_buf,
    );
    // Reusable dynamic vbuf for the per-frame icon quads (6 UiVertex per filled
    // slot). Sized for the open inventory + craft/chest slots with headroom; grown
    // to fit if ever exceeded (never a hard cap that drops the batch).
    let icon_quad_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("icon quad vbuf"),
        size: super::pipeline::MAX_UI_VERTICES * std::mem::size_of::<UiVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Data-driven GUI textures: upload each baked PNG (panel + optional hover +
    // each dynamic overlay) into its own bind group (reusing the gui-atlas bind
    // layout) keyed by GuiTexId. Loaded from disk at runtime so re-baking +
    // restarting picks them up with no recompile. See `gui_def`.
    let load_gui_bind = |path: &std::path::Path| -> Option<wgpu::BindGroup> {
        let bytes = std::fs::read(path).ok()?;
        let (_tex, view, sampler) = create_gui_panel(&device, &queue, &bytes);
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gui texture bind"),
            layout: &pipelines.atlas_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        }))
    };
    let mut gui_textures = std::collections::HashMap::new();
    for (kind, path) in super::gui_def::baked_panels() {
        if let Some(bind) = load_gui_bind(&path) {
            gui_textures.insert(GuiTexId::Panel(kind), bind);
        }
    }
    for (kind, path) in super::gui_def::baked_hovers() {
        if let Some(bind) = load_gui_bind(&path) {
            gui_textures.insert(GuiTexId::Hover(kind), bind);
        }
    }
    for (kind, tag, path) in super::gui_def::baked_overlays() {
        if let Some(bind) = load_gui_bind(&path) {
            gui_textures.insert(GuiTexId::Overlay(kind, tag), bind);
        }
    }
    let new_ui_quad_vbuf = |label| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: super::pipeline::MAX_UI_VERTICES * std::mem::size_of::<UiVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let ui_panel_vbuf = new_ui_quad_vbuf("ui panel vbuf");
    let ui_overlay_vbuf = new_ui_quad_vbuf("ui overlay vbuf");
    let ui_hover_vbuf = new_ui_quad_vbuf("ui hover vbuf");

    Renderer {
        surface,
        device,
        queue,
        config,
        atlas_texture,
        atlas_view,
        atlas_sampler,
        sky_pipe: pipelines.sky_pipe,
        sky_bind: pipelines.sky_bind,
        opaque_pipe: pipelines.opaque_pipe,
        transparent_pipe: pipelines.transparent_pipe,
        outline_pipe: pipelines.outline_pipe,
        outline_bind: pipelines.outline_bind,
        outline_vbuf: pipelines.outline_vbuf,
        outline_vertex_count: 0,
        crosshair_pipe: pipelines.crosshair_pipe,
        crosshair_vbuf: pipelines.crosshair_vbuf,
        crosshair_vertex_count: 0,
        crosshair_drawn_size: (0, 0),
        selection: None,
        selection_drawn: None,
        uniform_buf,
        uniform_bind: pipelines.uniform_bind,
        atlas_bind: pipelines.atlas_bind,
        model3d_hand_pipe: pipelines.model3d_hand_pipe,
        model3d_mvp_buf: pipelines.model3d_mvp_buf,
        model3d_mvp_bind: pipelines.model3d_mvp_bind,
        model3d_vbuf: pipelines.model3d_vbuf,
        model3d_ibuf: pipelines.model3d_ibuf,
        item3d_pipe: pipelines.item3d_pipe,
        item3d_mvp_bind: pipelines.item3d_mvp_bind,
        item3d_vbuf: pipelines.item3d_vbuf,
        item3d_verts: Vec::new(),
        item3d_vertex_count: 0,
        held_is_model: false,
        hand_index_count: 0,
        hand_verts: Vec::new(),
        hand_indices: Vec::new(),
        break_draw: DynamicDraw::new(
            pipelines.break_pipe,
            pipelines.break_vbuf,
            pipelines.break_ibuf,
            super::pipeline::MAX_BREAK_VERTICES,
            super::pipeline::MAX_BREAK_INDICES,
        ),
        item_entity_draw: DynamicDraw::new(
            item_entity_pipe,
            pipelines.item_entity_vbuf,
            pipelines.item_entity_ibuf,
            super::pipeline::MAX_ITEM_ENTITY_VERTICES,
            super::pipeline::MAX_ITEM_ENTITY_INDICES,
        ),
        chest_draw: DynamicDraw::new(
            chest_pipe,
            pipelines.chest_vbuf,
            pipelines.chest_ibuf,
            super::pipeline::MAX_CHEST_VERTICES,
            super::pipeline::MAX_CHEST_INDICES,
        ),
        door_draw: DynamicDraw::new(
            door_pipe,
            pipelines.door_vbuf,
            pipelines.door_ibuf,
            super::pipeline::MAX_DOOR_VERTICES,
            super::pipeline::MAX_DOOR_INDICES,
        ),
        mob_gpu,
        model_pipe: model_pipe.clone(),
        model_atlas_texture,
        model_atlas_view,
        model_atlas_sampler,
        model_atlas_bind,
        item_model_entity_draw: DynamicDraw::new(
            model_pipe,
            item_model_entity_vbuf,
            item_model_entity_ibuf,
            super::pipeline::MAX_MOB_VERTICES,
            super::pipeline::MAX_MOB_INDICES,
        ),
        item_model_entity_verts: Vec::new(),
        item_model_entity_indices: Vec::new(),
        particle_draw: DynamicVertexDraw::new(
            pipelines.particle_pipe,
            pipelines.particle_vbuf,
            pipelines.particle_ibuf,
            super::particles::MAX_PARTICLE_VERTICES as u64,
        ),
        depth,
        chunk_meshes: HashMap::new(),
        draw_order: Vec::new(),
        frustum: Frustum::permissive(),
        cam_pos: glam::Vec3::ZERO,
        section_visibility: SectionVisibilityCache::default(),
        clear_color: [0.60, 0.82, 1.00],
        last_stats: RenderStats::default(),
        break_overlay: None,
        held_item: HeldItemView::default(),
        held_item_anim: HeldItemAnimator::default(),
        held_item_skylight: super::lighting::FULL_SKYLIGHT,
        held_item_warm: 0,
        item_entities: Vec::new(),
        particles: Vec::new(),
        model_particles: Vec::new(),
        particle_block_vertex_count: 0,
        ui: UiSnapshot::default(),
        billboard_basis: BillboardBasis {
            right: glam::Vec3::X,
            up: glam::Vec3::Y,
        },
        item_entity_verts: Vec::new(),
        item_entity_indices: Vec::new(),
        item_entity_visible: Vec::new(),
        chests: Vec::new(),
        chest_visible: Vec::new(),
        doors: Vec::new(),
        door_visible: Vec::new(),
        mobs: Vec::new(),
        particle_verts: Vec::new(),
        ui_pipe: pipelines.ui_pipe,
        gui_textures,
        ui_solid_vbuf: pipelines.ui_vbuf,
        ui_dim_vertex_count: 0,
        ui_count_vertex_count: 0,
        ui_drag_count_vertex_count: 0,
        ui_panel_vbuf,
        ui_panel_vertex_count: 0,
        ui_overlay_vbuf,
        ui_overlay_vertex_count: 0,
        ui_hover_vbuf,
        ui_hover_vertex_count: 0,
        icon_atlas,
        icon_quad_vbuf,
        icon_quad_verts: Vec::new(),
        icon_quad_vertex_count: 0,
        drag_icon_quad_vertex_count: 0,
        ui_build: UiBuild::default(),
        hand_vertex_count: 0,
    }
}

impl Renderer {
    /// The current surface size in physical pixels `(width, height)` — the same
    /// coordinate space the UI layout (`render::ui`) and cursor hit-testing use.
    #[inline]
    pub fn screen_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = create_depth(&self.device, width, height);
        self.crosshair_drawn_size = (0, 0);
    }

    pub fn update_uniforms(
        &mut self,
        cam: &Camera,
        fog_color: [f32; 3],
        time: f32,
        underwater: bool,
    ) {
        let view_proj = cam.view_proj();
        let inv_view_proj = view_proj.inverse();
        // Refresh the culling frustum from the same matrix the GPU will use.
        self.frustum = Frustum::from_view_proj(view_proj);
        self.cam_pos = cam.pos;
        // Camera right/up axes for world-space billboards (item sprites + dust):
        // a quad spanned by these always faces the viewer.
        self.billboard_basis = BillboardBasis {
            right: cam.right(),
            up: cam.up(),
        };
        self.clear_color = fog_color;
        let (fog_start, fog_end) = if underwater {
            (UNDERWATER_FOG_START, UNDERWATER_FOG_END)
        } else {
            (FOG_START, FOG_END)
        };
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            cam_pos: [cam.pos.x, cam.pos.y, cam.pos.z, 0.0],
            // fog.z = animation time (caustics), fog.w = underwater flag.
            fog: [fog_start, fog_end, time, if underwater { 1.0 } else { 0.0 }],
            fog_color: [fog_color[0], fog_color[1], fog_color[2], 1.0],
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            water_anim: crate::atlas::water_anim_uniform(),
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[u]));
    }

    /// Set (or clear) the target highlighted by the selection outline. Cheap: the
    /// vertex buffer is only re-uploaded in `render` when the target changes.
    pub fn set_selection(&mut self, shape: Option<SelectionShape>) {
        self.selection = shape;
    }

    /// Store the block-break overlay to draw this frame (or `None` to clear).
    pub fn set_break_overlay(&mut self, v: Option<BreakOverlayView>) {
        self.break_overlay = v;
    }

    /// Advance and store the first-person held-item / hand state for this frame.
    pub fn set_held_item(&mut self, v: HeldItemFrame) {
        self.held_item = self.held_item_anim.update(v);
    }

    /// Store the combined light + warm-tint amount to apply to the first-person hand
    /// / held item (so it brightens AND warms near torches/furnaces).
    pub fn set_held_item_light(&mut self, skylight: u8, warm: u8) {
        self.held_item_skylight = skylight.min(super::lighting::FULL_SKYLIGHT);
        self.held_item_warm = warm;
    }

    /// Store the dropped item-entities to draw this frame. Reuses the existing
    /// `Vec` capacity (clear + extend) to avoid per-frame reallocation.
    pub fn set_item_entities(&mut self, v: &[ItemEntityInstance]) {
        self.item_entities.clear();
        self.item_entities.extend_from_slice(v);
    }

    /// Store the placed chests to draw this frame. Reuses the existing `Vec`
    /// capacity (clear + extend) to avoid per-frame reallocation.
    pub fn set_chests(&mut self, v: &[ChestInstance]) {
        self.chests.clear();
        self.chests.extend_from_slice(v);
    }

    /// Store the placed doors to draw this frame. Reuses the existing `Vec` capacity
    /// (clear + extend) to avoid per-frame reallocation.
    pub fn set_doors(&mut self, v: &[DoorInstance]) {
        self.doors.clear();
        self.doors.extend_from_slice(v);
    }

    /// Store the mobs to draw this frame (already interpolated by the scene adapter).
    /// Reuses the existing `Vec` capacity.
    pub fn set_mobs(&mut self, v: &[MobRenderInstance]) {
        self.mobs.clear();
        self.mobs.extend_from_slice(v);
    }

    /// Store the block-atlas particle cubes to draw this frame. Reuses capacity.
    pub fn set_particles(&mut self, v: &[ParticleInstance]) {
        self.particles.clear();
        self.particles.extend_from_slice(v);
    }

    /// Store the model-atlas particle cubes (bbmodel-block flecks) for this frame; they
    /// bake into the same particle vbuf after the block cubes and draw with the model
    /// atlas bound. Reuses capacity.
    pub fn set_model_particles(&mut self, v: &[ParticleInstance]) {
        self.model_particles.clear();
        self.model_particles.extend_from_slice(v);
    }

    /// Snapshot the UI/inventory bits needed for this frame's UI pass. Extracts
    /// the small flat state (open flag, screen, cursor, 36 slot stacks, active
    /// slot, cursor stack) into owned `Renderer` state so the renderer never
    /// holds a borrow of the game `Inventory`.
    pub fn set_ui(&mut self, v: UiFrame) {
        self.ui.open = v.open;
        self.ui.kind = v.kind;
        self.ui.screen = v.screen;
        self.ui.cursor_px = v.cursor_px;
        self.ui.active = v.inv.active_slot();
        for (i, slot) in self.ui.slots.iter_mut().enumerate() {
            *slot = v.inv.slot(i).map(|s| (s.item, s.count));
        }
        self.ui.cursor = v.inv.cursor().map(|s| (s.item, s.count));
        // Snapshot the crafting grid (cells past the live range stay cleared).
        for (i, cell) in self.ui.craft.iter_mut().enumerate() {
            *cell = v.craft.get(i).copied().flatten().map(|s| (s.item, s.count));
        }
        self.ui.result = v.craft_result.map(|s| (s.item, s.count));
        self.ui.furnace = v.furnace;
        self.ui.chest = v.chest;
        self.ui.workbench = v.workbench;
    }

    /// Is this chunk mesh's bounding box inside the current view frustum?
    #[inline]
    fn chunk_visible(&self, gm: &GpuMesh) -> bool {
        let (ox, oz) = gm.origin;
        let min = glam::Vec3::new(ox as f32, 0.0, oz as f32);
        let max = glam::Vec3::new((ox + 16) as f32, CHUNK_SY as f32, (oz + 16) as f32);
        self.frustum.aabb_visible(min, max)
    }

    /// Synchronize GPU meshes with the World's CPU meshes.
    pub fn sync_meshes(&mut self, world: &mut World) {
        // Drop GPU meshes whose CPU chunk is gone — checked through the world's
        // mesh accessor so no per-frame scratch set is allocated.
        self.chunk_meshes.retain(|p, _| world.has_mesh(*p));
        // Upload only meshes marked dirty by the world (newly built/changed) or
        // missing on the GPU; clear each CPU dirty flag as it is uploaded. Existing
        // unchanged meshes are left on the GPU untouched.
        for (pos, mesh) in world.iter_meshes_mut() {
            let need_upload = !self.chunk_meshes.contains_key(&pos) || mesh.mesh_dirty;
            if need_upload {
                let gm = upload_mesh(&self.device, mesh, pos);
                self.chunk_meshes.insert(pos, gm);
                mesh.mesh_dirty = false;
            }
        }
    }

    pub fn update_section_visibility(&mut self, world: &mut World) {
        self.section_visibility.update(world, self.cam_pos);
    }

    /// Build + upload this frame's UI geometry from the [`UiBuild`] that
    /// [`build_ui`] fills. Each quad group goes to its own buffer / range so the UI
    /// pass binds the right texture per group:
    /// - `ui_solid_vbuf`: dim backdrop `[0, dim)`, then stack counts, then drag
    ///   counts — all solid-color, drawn with the icon-atlas bind (the solid
    ///   sentinel skips the sampler).
    /// - `ui_panel_vbuf` / `ui_overlay_vbuf` / `ui_hover_vbuf`: the baked panel,
    ///   the dynamic overlays, and the hover highlight (each its own texture).
    /// - `icon_quad_vbuf`: one textured quad per filled slot sampling the item's
    ///   pre-baked icon-atlas cell — normal icons then cursor-held icons.
    fn build_ui_frame(&mut self) {
        self.ui_dim_vertex_count = 0;
        self.ui_count_vertex_count = 0;
        self.ui_drag_count_vertex_count = 0;
        self.ui_panel_vertex_count = 0;
        self.ui_overlay_vertex_count = 0;
        self.ui_hover_vertex_count = 0;
        self.icon_quad_vertex_count = 0;
        self.drag_icon_quad_vertex_count = 0;

        // Disjoint-field borrow: `build_ui` reads the snapshot and writes the
        // scratch `UiBuild`, both distinct from the GPU buffers used below.
        build_ui(&self.ui, &mut self.ui_build);

        let cap = super::pipeline::MAX_UI_VERTICES as usize;
        let vsize = std::mem::size_of::<UiVertex>();

        // Solid quads packed into one buffer: dim backdrop, then normal stack
        // counts, then the cursor-held count (drawn after the cursor icon).
        let dim = &self.ui_build.dim;
        let counts = &self.ui_build.counts;
        let drag_counts = &self.ui_build.drag_counts;
        if !dim.is_empty() && dim.len() <= cap {
            self.queue.write_buffer(&self.ui_solid_vbuf, 0, bytemuck::cast_slice(dim));
            self.ui_dim_vertex_count = dim.len() as u32;
        }
        let mut off = self.ui_dim_vertex_count as usize;
        if !counts.is_empty() && off + counts.len() <= cap {
            self.queue.write_buffer(&self.ui_solid_vbuf, (off * vsize) as u64, bytemuck::cast_slice(counts));
            self.ui_count_vertex_count = counts.len() as u32;
            off += counts.len();
        }
        if !drag_counts.is_empty() && off + drag_counts.len() <= cap {
            self.queue.write_buffer(&self.ui_solid_vbuf, (off * vsize) as u64, bytemuck::cast_slice(drag_counts));
            self.ui_drag_count_vertex_count = drag_counts.len() as u32;
        }

        // Baked panel + dynamic overlays + hover highlight, each its own buffer.
        let panel = &self.ui_build.panel;
        if !panel.is_empty() && panel.len() <= cap {
            self.queue.write_buffer(&self.ui_panel_vbuf, 0, bytemuck::cast_slice(panel));
            self.ui_panel_vertex_count = panel.len() as u32;
        }
        let overlays = &self.ui_build.overlays;
        if !overlays.is_empty() && overlays.len() <= cap {
            self.queue.write_buffer(&self.ui_overlay_vbuf, 0, bytemuck::cast_slice(overlays));
            self.ui_overlay_vertex_count = overlays.len() as u32;
        }
        let hover = &self.ui_build.hover;
        if !hover.is_empty() && hover.len() <= cap {
            self.queue.write_buffer(&self.ui_hover_vbuf, 0, bytemuck::cast_slice(hover));
            self.ui_hover_vertex_count = hover.len() as u32;
        }

        // Per-slot item icons: resolve each recorded `(item, slot rect)` to the item's
        // pre-baked icon-atlas cell and emit a textured quad (6 verts) — slot rect →
        // NDC, cell rect → uv, white tint (so the quad samples the atlas, not the solid
        // sentinel). Normal icons draw in the UI pass; cursor-held icons are appended
        // to the same buffer but drawn later, after normal stack-count overlays.
        let screen = self.ui.screen;
        let mut verts = std::mem::take(&mut self.icon_quad_verts);
        verts.clear();
        if screen.0 != 0 && screen.1 != 0 {
            for &(item, r) in &self.ui_build.icon_quads {
                let [u0, v0, u1, v1] = self.icon_atlas.cell_uv(item);
                super::ui::push_quad_uv(
                    &mut verts,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [u0, v0],
                    [u1, v1],
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
            // Greyed (semi-transparent) icons — workbench results not yet craftable.
            // Same icon-atlas quad, drawn at reduced alpha so the panel shows through.
            for &(item, r) in &self.ui_build.dim_icon_quads {
                let [u0, v0, u1, v1] = self.icon_atlas.cell_uv(item);
                super::ui::push_quad_uv(
                    &mut verts,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [u0, v0],
                    [u1, v1],
                    [1.0, 1.0, 1.0, 0.35],
                );
            }
            let normal_icon_vertex_count = verts.len() as u32;
            for &(item, r) in &self.ui_build.drag_icon_quads {
                let [u0, v0, u1, v1] = self.icon_atlas.cell_uv(item);
                super::ui::push_quad_uv(
                    &mut verts,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [u0, v0],
                    [u1, v1],
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
            self.icon_quad_vertex_count = normal_icon_vertex_count;
            self.drag_icon_quad_vertex_count = verts.len() as u32 - normal_icon_vertex_count;
        }
        if !verts.is_empty() {
            // Icon-quad geometry is bounded by the visible slots but GROW the buffer to
            // fit rather than capping — a fixed cap that drops the batch when exceeded
            // would blank EVERY icon at once. Grow to the next power of two so it
            // doesn't reallocate every frame.
            let bytes = bytemuck::cast_slice::<_, u8>(verts.as_slice()).len() as u64;
            if bytes > self.icon_quad_vbuf.size() {
                self.icon_quad_vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("icon quad vbuf"),
                    size: bytes.next_power_of_two(),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            self.queue
                .write_buffer(&self.icon_quad_vbuf, 0, bytemuck::cast_slice(&verts));
        }
        self.icon_quad_verts = verts;
    }

    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        if self.crosshair_drawn_size != (self.config.width, self.config.height) {
            let verts = crosshair_vertices(self.config.width, self.config.height);
            self.crosshair_vertex_count = verts.count;
            if verts.count > 0 {
                self.queue.write_buffer(
                    &self.crosshair_vbuf,
                    0,
                    bytemuck::cast_slice(&verts.vertices[..verts.count as usize]),
                );
            }
            self.crosshair_drawn_size = (self.config.width, self.config.height);
        }

        // Refresh the outline vertex buffer only when the target changed.
        if self.selection != self.selection_drawn {
            self.outline_vertex_count = 0;
            if let Some(shape) = self.selection {
                let outline = outline_vertices(shape);
                self.outline_vertex_count = outline.count;
                if outline.count > 0 {
                    self.queue.write_buffer(
                        &self.outline_vbuf,
                        0,
                        bytemuck::cast_slice(&outline.vertices[..outline.count as usize]),
                    );
                }
            }
            self.selection_drawn = self.selection;
        }

        // Build + upload the first-person hand geometry for this frame. The hand
        // uses its own fixed perspective (it is drawn over the world, no depth),
        // so the MVP is computed entirely here from the framebuffer aspect and the
        // App-supplied swing/place phases, then written to MVP slot 0. The dynamic
        // vbuf/ibuf are rewritten in place (no per-frame allocation).
        self.hand_index_count = 0;
        self.hand_vertex_count = 0;
        {
            let aspect = if self.config.height > 0 {
                self.config.width as f32 / self.config.height as f32
            } else {
                1.0
            };
            // Take the reusable hand staging out so `build_hand` can borrow them
            // mutably alongside the immutable `held_item` borrow, then restore.
            let mut hv = std::mem::take(&mut self.hand_verts);
            let mut hi = std::mem::take(&mut self.hand_indices);
            let mvp = build_hand_lit(
                &self.held_item,
                aspect,
                self.held_item_skylight,
                self.held_item_warm,
                &mut hv,
                &mut hi,
            );
            if !hi.is_empty() {
                self.queue
                    .write_buffer(&self.model3d_vbuf, 0, bytemuck::cast_slice(&hv));
                self.queue
                    .write_buffer(&self.model3d_ibuf, 0, bytemuck::cast_slice(&hi));
                // MVP slot 0: a 64-byte mat4 at offset 0 of the 256-aligned buffer.
                self.queue.write_buffer(
                    &self.model3d_mvp_buf,
                    0,
                    bytemuck::cast_slice(&mvp.to_cols_array()),
                );
                self.hand_index_count = hi.len() as u32;
                self.hand_vertex_count = hv.len() as u32;
            }
            self.hand_verts = hv;
            self.hand_indices = hi;
        }

        // Build + upload the EXTRUDED held item (sprite-kind: flowers / future
        // tools), drawn by the dedicated item3d pipeline in the hand pass. Mutually
        // exclusive with the model3d hand geometry (a sprite emits none above), so
        // its MVP reuses slot 0 of `model3d_mvp_buf`. The item3d vbuf is rewritten
        // in place (no per-frame allocation beyond capacity).
        self.item3d_vertex_count = 0;
        self.held_is_model = false;
        {
            let aspect = if self.config.height > 0 {
                self.config.width as f32 / self.config.height as f32
            } else {
                1.0
            };
            if let Some((kind, mvp)) = super::hand::held_model(&self.held_item, aspect) {
                // A held bbmodel block: bake its real model (model atlas) into the item3d
                // vbuf and draw it through the item3d pipeline bound to the MODEL atlas.
                // item3d is non-indexed, so expand the baked indexed mesh to a triangle
                // list. Mutually exclusive with a held sprite (one render kind).
                let mut iv = std::mem::take(&mut self.item3d_verts);
                iv.clear();
                let (mut tv, mut ti) = (Vec::new(), Vec::new());
                super::item_model::build_block_model_item(
                    kind,
                    glam::Mat4::IDENTITY,
                    self.held_item_skylight,
                    self.held_item_warm,
                    None,
                    &mut tv,
                    &mut ti,
                );
                for &idx in &ti {
                    iv.push(tv[idx as usize]);
                }
                let cap = super::pipeline::MAX_ITEM3D_VERTICES as usize;
                if !iv.is_empty() && iv.len() <= cap {
                    self.queue
                        .write_buffer(&self.item3d_vbuf, 0, bytemuck::cast_slice(&iv));
                    self.queue.write_buffer(
                        &self.model3d_mvp_buf,
                        0,
                        bytemuck::cast_slice(&mvp.to_cols_array()),
                    );
                    self.item3d_vertex_count = iv.len() as u32;
                    self.held_is_model = true;
                }
                self.item3d_verts = iv;
            } else if let Some((tile, mvp)) = super::hand::held_sprite(&self.held_item, aspect) {
                let mut iv = std::mem::take(&mut self.item3d_verts);
                let count = super::item_model::build_extruded_item_lit(
                    tile,
                    self.held_item_skylight,
                    &mut iv,
                );
                // Warm the extruded held sprite by the block-light at the player, to
                // match the warm tint static blocks + the model3d hand take near a
                // torch/furnace. (Item entities reuse this builder but aren't warmed.)
                if self.held_item_warm > 0 {
                    let w = self.held_item_warm as f32 / 255.0;
                    for v in iv.iter_mut() {
                        v.tint = crate::torch::warm_tint(v.tint, w);
                    }
                }
                let cap = super::pipeline::MAX_ITEM3D_VERTICES as usize;
                if count > 0 && iv.len() <= cap {
                    self.queue
                        .write_buffer(&self.item3d_vbuf, 0, bytemuck::cast_slice(&iv));
                    // MVP slot 0 (the model3d hand slot is free for a held sprite).
                    self.queue.write_buffer(
                        &self.model3d_mvp_buf,
                        0,
                        bytemuck::cast_slice(&mvp.to_cols_array()),
                    );
                    self.item3d_vertex_count = count;
                }
                self.item3d_verts = iv;
            }
        }

        // Build the UI geometry for this frame: gui-atlas quads (background +
        // digit overlay) into the UI vbuf, and one textured quad per filled slot
        // (sampling the pre-baked icon atlas) into the icon-quad vbuf. All buffers
        // are rewritten in place — capacity is retained across frames.
        self.build_ui_frame();

        // Bake the dynamic world subsystems. Item-entity, chest, and break-overlay
        // each clear-and-refill the SAME shared CPU scratch (`item_entity_verts` /
        // `item_entity_indices`) in this exact order — `bake` (clear count → build
        // → bounds-check → upload to that subsystem's OWN fixed buffers → store
        // count) runs sequentially, never aliasing two GPU buffers at once. Each
        // subsystem keeps its OWN buffer caps (item-entity vs chest sized apart so
        // a wall of chests can't make dropped items vanish).

        // Item entities (spinning cubes / sprite billboards), frustum-culled so
        // off-screen drops cost nothing. Drawn by the EXISTING opaque pipeline.
        self.item_entity_visible.clear();
        for inst in &self.item_entities {
            // ~0.5 m cull box around the item centre.
            let c = inst.pos;
            let min = c - glam::Vec3::splat(0.5);
            let max = c + glam::Vec3::new(0.5, 1.0, 0.5);
            if self.frustum.aabb_visible(min, max) {
                self.item_entity_visible.push(*inst);
            }
        }
        let basis = self.billboard_basis;
        let visible = &self.item_entity_visible;
        self.item_entity_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| build_item_entities(visible, basis, verts, indices),
        );
        // Dropped bbmodel items (their own model atlas), baked from the same visible set.
        let visible = &self.item_entity_visible;
        self.item_model_entity_draw.bake(
            &self.queue,
            &mut self.item_model_entity_verts,
            &mut self.item_model_entity_indices,
            |verts, indices| super::item_entity::build_item_model_entities(visible, verts, indices),
        );

        // Chests (inset body + hinged lid), frustum-culled like item entities and
        // reusing their CPU scratch. Drawn by the EXISTING opaque pipeline.
        self.chest_visible.clear();
        for inst in &self.chests {
            // Cull box: the block cell, expanded upward to include the open lid.
            let min = inst.pos;
            let max = inst.pos + glam::Vec3::new(1.0, 2.0, 1.0);
            if self.frustum.aabb_visible(min, max) {
                self.chest_visible.push(*inst);
            }
        }
        let chest_visible = &self.chest_visible;
        self.chest_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| build_chests(chest_visible, verts, indices),
        );

        // Doors (2-tall hinged slab), frustum-culled and baked exactly like chests,
        // reusing the same CPU scratch. Drawn by the EXISTING opaque pipeline.
        self.door_visible.clear();
        for inst in &self.doors {
            // Cull box: the door's two-cell column (its swung slab stays within it).
            let min = inst.pos;
            let max = inst.pos + glam::Vec3::new(1.0, 2.0, 1.0);
            if self.frustum.aabb_visible(min, max) {
                self.door_visible.push(*inst);
            }
        }
        let door_visible = &self.door_visible;
        self.door_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| build_doors(door_visible, verts, indices),
        );

        // Mobs (animated entity models), grouped by species and frustum-culled, baked
        // into each species' OWN `ItemVertex` buffers (a different vertex type from the
        // packed block vertex). Each instance is posed by the walk animation at its
        // `anim_time` when moving, else the model's rest pose.
        for g in &mut self.mob_gpu {
            g.visible.clear();
        }
        for inst in &self.mobs {
            // Cull box: ~0.5 m around the feet, expanded up for the standing body. A
            // killed mob is flung from its (frozen) death point and tumbles across the
            // ground, so use a generous box while it's ragdolling so the flying corpse
            // doesn't pop out of view.
            let pad = if inst.ragdoll.is_some() {
                glam::Vec3::splat(6.0)
            } else {
                glam::Vec3::new(0.5, 1.2, 0.5)
            };
            let min = inst.pos - pad;
            let max = inst.pos + pad;
            if self.frustum.aabb_visible(min, max) {
                self.mob_gpu[inst.kind as usize].visible.push(inst.clone());
            }
        }
        let queue = &self.queue;
        for g in &mut self.mob_gpu {
            let model = g.model;
            let scale = g.scale;
            let visible = &g.visible;
            g.draw
                .bake(queue, &mut g.verts, &mut g.indices, |verts, indices| {
                    build_mob_instances(model, scale, visible, verts, indices)
                });
        }

        // Break-overlay (destroy crack) cube, when a block is targeted.
        let break_overlay = self.break_overlay;
        self.break_draw.bake(
            &self.queue,
            &mut self.item_entity_verts,
            &mut self.item_entity_indices,
            |verts, indices| match break_overlay {
                Some(view) => build_break_overlay(&view, verts, indices),
                None => {
                    verts.clear();
                    indices.clear();
                    0
                }
            },
        );

        // Tiny 3D particle cubes into the reusable vbuf (static quad ibuf): block-atlas
        // flecks first, then bbmodel-block (model-atlas) flecks, so the draw splits at one
        // contiguous index boundary (`particle_block_vertex_count`).
        let particles = &self.particles;
        let model_particles = &self.model_particles;
        let mut block_v = 0u32;
        self.particle_draw
            .bake(&self.queue, &mut self.particle_verts, |verts| {
                let (total, nb) = build_particles_split(particles, model_particles, verts);
                block_v = nb;
                total
            });
        self.particle_block_vertex_count = if self.particle_draw.vertex_count == 0 {
            0
        } else {
            block_v
        };

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        // Frustum-cull + depth-sort the visible chunks once. The opaque pass
        // draws nearest-first so the GPU's early-Z rejects occluded fragments
        // before the (texture + tint + fog) fragment shader runs, cutting
        // overdraw, which is the dominant GPU cost in dense voxel terrain. The
        // transparent pass draws farthest-first for correct back-to-front alpha.
        let cam = self.cam_pos;
        let section_culling_active = self.section_visibility.is_active();
        let mut frustum_chunks = 0u32;
        // Reusable draw order: `(dist_sq, ChunkPos)` keys (not `&GpuMesh`, so it can
        // live on `Renderer` across frames). Taken out so it can be filled while
        // `self.chunk_meshes`/`section_visibility` are read, then restored — the
        // draw loops look the mesh up by key. Cleared + refilled, capacity retained.
        let mut order = std::mem::take(&mut self.draw_order);
        order.clear();
        // Whether any VISIBLE chunk carries model geometry — folded into the draw-order
        // walk so the model pass needs no separate scan over all loaded chunks to decide
        // whether to run (and never opens an empty pass for off-screen model blocks).
        let mut any_model_visible = false;
        for gm in self.chunk_meshes.values() {
            if !self.chunk_visible(gm) {
                continue;
            }
            frustum_chunks += 1;
            if section_culling_active && self.section_visibility.chunk_mask(gm.pos).is_none() {
                continue;
            }
            any_model_visible |= gm.model_idx_count > 0;
            let (ox, oz) = gm.origin;
            let c = glam::Vec3::new(ox as f32 + 8.0, CHUNK_SY as f32 * 0.5, oz as f32 + 8.0);
            order.push(((cam - c).length_squared(), gm.pos));
        }
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut stats = RenderStats {
            frustum_chunks,
            drawn_chunks: order.len() as u32,
            visible_sections: self.section_visibility.visible_section_count(),
            section_culling_active,
            ..Default::default()
        };
        let cc = self.clear_color;
        // SKY PASS: full-screen background triangle. The ONLY pass that CLEARS
        // color (to the fog colour); no depth attachment.
        {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "sky pass",
                wgpu::LoadOp::Clear(wgpu::Color {
                    r: cc[0] as f64,
                    g: cc[1] as f64,
                    b: cc[2] as f64,
                    a: 1.0,
                }),
                None,
            );
            pass.set_pipeline(&self.sky_pipe);
            pass.set_bind_group(0, &self.sky_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        // OPAQUE PASS: the visible chunk terrain, near→far for early-Z. CLEARS the
        // depth buffer (the first depth user this frame); loads color over the sky.
        {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "opaque pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Clear(1.0)),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_pipeline(&self.opaque_pipe);
            for (dist_sq, pos) in order.iter() {
                let gm = &self.chunk_meshes[pos];
                // near -> far (early-Z)
                let use_far_leaf_lod =
                    far_leaf_lod_active(*dist_sq, gm.origin, gm.far_opaque_idx_count > 0);
                let (vbuf, ibuf, idx_count, sections) = if use_far_leaf_lod {
                    (
                        &gm.far_opaque_vbuf,
                        &gm.far_opaque_ibuf,
                        gm.far_opaque_idx_count,
                        &gm.far_opaque_sections,
                    )
                } else {
                    (
                        &gm.opaque_vbuf,
                        &gm.opaque_ibuf,
                        gm.opaque_idx_count,
                        &gm.opaque_sections,
                    )
                };
                if idx_count == 0 {
                    continue;
                }
                if let (Some(vb), Some(ib)) = (vbuf, ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    if let Some(mask) = self.section_visibility.chunk_mask(gm.pos) {
                        let ranges =
                            section_draw_ranges(self.frustum, gm.origin, idx_count, sections, mask);
                        if ranges.is_empty() {
                            continue;
                        }
                        for (start, end) in ranges.iter() {
                            stats.opaque_draws += 1;
                            pass.draw_indexed(start..end, 0, 0..1);
                        }
                        stats.opaque_indices += ranges.submitted as u64;
                        stats.section_culled_indices += (idx_count - ranges.submitted) as u64;
                    } else {
                        stats.opaque_draws += 1;
                        stats.opaque_indices += idx_count as u64;
                        pass.draw_indexed(0..idx_count, 0, 0..1);
                    }
                }
            }
        }
        // MODEL PASS: bbmodel-block geometry (explicit-UV, sampling the model atlas),
        // drawn per visible chunk with the mob pipeline (own texture + the same
        // underwater/fog the world uses) over depth from the opaque pass — so a placed
        // model occludes and is occluded by terrain like any block. Most chunks have no
        // model geometry, so this is usually a no-op loop.
        if any_model_visible || self.item_model_entity_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "model pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.model_atlas_bind, &[]);
            pass.set_pipeline(&self.model_pipe);
            for (_, pos) in order.iter() {
                let gm = &self.chunk_meshes[pos];
                if gm.model_idx_count == 0 {
                    continue;
                }
                if let (Some(vb), Some(ib)) = (&gm.model_vbuf, &gm.model_ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..gm.model_idx_count, 0, 0..1);
                }
            }
            // Dropped bbmodel items (world-space, same model atlas + pipeline).
            self.item_model_entity_draw.draw(&mut pass);
        }
        // ITEM-ENTITY PASS (§8 2b): dropped items as full-bright spinning cubes /
        // sprite billboards, drawn by the EXISTING opaque pipeline (no new
        // pipeline) with the SAME uniform + atlas binds. Load color + depth,
        // depth test + write so items occlude and are occluded by terrain.
        if self.item_entity_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "item entity pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            self.item_entity_draw.draw(&mut pass);
        }
        // CHEST + DOOR PASS: placed chests (inset body + hinged lid) and doors (2-tall
        // hinged slab) drawn as full opaque geometry by the EXISTING opaque pipeline
        // with the same uniform + atlas binds, loading color + depth so they occlude and
        // are occluded by terrain — exactly like the item-entity pass above.
        if self.chest_draw.index_count > 0 || self.door_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "chest+door pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            self.chest_draw.draw(&mut pass);
            self.door_draw.draw(&mut pass);
        }
        // MOB PASS: animated entity models, one draw per visible species. Loads color
        // + depth (test + WRITE) so mobs occlude and are occluded by terrain — like
        // the item-entity / chest passes — but binds each species' OWN texture at
        // group(1) (not the block atlas); the mob pipeline (set by each DynamicDraw)
        // uses explicit-UV vertices so a model's arbitrary sub-rect UVs sample its
        // own sheet.
        if self.mob_gpu.iter().any(|g| g.draw.index_count > 0) {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "mob pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            for g in &self.mob_gpu {
                if g.draw.index_count == 0 {
                    continue;
                }
                pass.set_bind_group(1, &g.bind, &[]);
                g.draw.draw(&mut pass);
            }
        }
        // BREAK-OVERLAY PASS: the destroy crack over the targeted block. Drawn
        // BEFORE the transparent water pass — it is a decal on the OPAQUE block, so
        // water must be able to blend in front of it (a crack on a submerged block
        // shows THROUGH the water, not over it). MULTIPLY blend; depth LessEqual /
        // no-write over a cube built COINCIDENT with the block faces (no inflation,
        // so the decal never misaligns), with a small polygon offset toward the
        // camera (BREAK_DEPTH_BIAS) so it wins the depth tie cleanly. Reuses
        // uniform_bind (view_proj + uv_rects) + atlas_bind.
        if self.break_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "break overlay pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            self.break_draw.draw(&mut pass);
        }
        // PARTICLE PASS (§8 3b): tiny 3D terrain particle cubes. Drawn BEFORE the
        // transparent water pass (but after the break overlay, so they sit in front
        // of the crack): they are alpha-CUTOUT solids that DEPTH-TEST + DEPTH-WRITE,
        // so water blends over the ones behind it (underwater dust reads as
        // submerged) while ones in front of the water still occlude it. Reuses
        // uniform_bind + atlas_bind. 24 verts / 36 indices per cube.
        if self.particle_draw.vertex_count > 0 {
            let verts_per_cube = super::particles::VERTS_PER_CUBE as u32;
            let idx_per_cube = super::particles::INDICES_PER_CUBE as u32;
            // Cube boundaries: block flecks occupy [0..block_cubes), model flecks the rest.
            let total_cubes = self.particle_draw.vertex_count / verts_per_cube;
            let block_cubes = self.particle_block_vertex_count / verts_per_cube;
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "particle pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            // Block-atlas flecks: the leading index range via the standard draw.
            if block_cubes > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.particle_draw
                    .draw(&mut pass, block_cubes * idx_per_cube);
            }
            // Model-atlas flecks (bbmodel blocks): the trailing index range, same vbuf with
            // the model atlas bound. Indices are absolute into the shared vbuf, so no base-
            // vertex offset is needed.
            if total_cubes > block_cubes {
                pass.set_bind_group(1, &self.model_atlas_bind, &[]);
                pass.set_pipeline(&self.particle_draw.pipeline);
                pass.set_vertex_buffer(0, self.particle_draw.vbuf.slice(..));
                pass.set_index_buffer(self.particle_draw.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    block_cubes * idx_per_cube..total_cubes * idx_per_cube,
                    0,
                    0..1,
                );
            }
        }
        // TRANSPARENT PASS: water, far→near for correct back-to-front alpha. Loads
        // color + depth; depth test (no write) so it sorts behind solids.
        {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "transparent pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_pipeline(&self.transparent_pipe);
            for (_, pos) in order.iter().rev() {
                let gm = &self.chunk_meshes[pos];
                // far -> near (alpha order)
                if let (Some(vb), Some(ib)) = (&gm.transparent_vbuf, &gm.transparent_ibuf) {
                    if gm.transparent_idx_count == 0 {
                        continue;
                    }
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    if let Some(mask) = self.section_visibility.chunk_mask(gm.pos) {
                        let ranges = section_draw_ranges(
                            self.frustum,
                            gm.origin,
                            gm.transparent_idx_count,
                            &gm.transparent_sections,
                            mask,
                        );
                        if ranges.is_empty() {
                            continue;
                        }
                        for (start, end) in ranges.iter() {
                            stats.transparent_draws += 1;
                            pass.draw_indexed(start..end, 0, 0..1);
                        }
                        stats.transparent_indices += ranges.submitted as u64;
                        stats.section_culled_indices +=
                            (gm.transparent_idx_count - ranges.submitted) as u64;
                    } else {
                        stats.transparent_draws += 1;
                        stats.transparent_indices += gm.transparent_idx_count as u64;
                        pass.draw_indexed(0..gm.transparent_idx_count, 0, 0..1);
                    }
                }
            }
        }
        // Restore the reusable draw-order buffer (capacity retained for next frame).
        self.draw_order = order;
        // Selection outline, after particles: load color + depth, depth-test (no
        // write) so it draws over terrain/water at the targeted block but stays
        // occluded behind nearer geometry.
        if self.selection.is_some() && self.outline_vertex_count > 0 {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "outline pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_pipeline(&self.outline_pipe);
            pass.set_bind_group(0, &self.outline_bind, &[]);
            pass.set_vertex_buffer(0, self.outline_vbuf.slice(..));
            pass.draw(0..self.outline_vertex_count, 0..1);
        }
        // HAND PASS (§8 4c): the first-person held item / bare hand, drawn over the
        // world. Color Load; the world colour is already composited, so we attach
        // the main depth buffer with LoadOp::Clear(1.0) — clearing depth gives the
        // hand its own isolated depth space (it stays on top of the world and never
        // clips terrain) while still letting the held geometry SELF-SORT. The bare
        // arm + held block go through the depth-enabled model3d_hand pipeline
        // (slot 0 = the hand MVP); a held SPRITE goes through the (now depth-tested)
        // item3d pipeline (extruded, slot 0 = the item MVP — the model3d hand is
        // empty in that case, so slot 0 is free). They are mutually exclusive, but
        // both are drawn here so the pass is correct regardless.
        if self.hand_index_count > 0 || self.item3d_vertex_count > 0 {
            // NB: depth load-op is CLEAR(1.0) — this pass intentionally resets the
            // depth buffer so the hand self-sorts in isolation from the world.
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "hand pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Clear(1.0)),
            );
            // Bare arm / held block (model3d, depth-enabled hand variant).
            if self.hand_index_count > 0 {
                pass.set_pipeline(&self.model3d_hand_pipe);
                pass.set_bind_group(0, &self.model3d_mvp_bind, &[0]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.model3d_vbuf.slice(..));
                pass.set_index_buffer(self.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.hand_index_count, 0, 0..1);
            }
            // Extruded held sprite (block atlas) OR a held bbmodel block (model atlas) —
            // both ride the item3d pipeline (non-indexed triangle list, depth-tested).
            if self.item3d_vertex_count > 0 {
                pass.set_pipeline(&self.item3d_pipe);
                pass.set_bind_group(0, &self.item3d_mvp_bind, &[0]);
                let atlas = if self.held_is_model {
                    &self.model_atlas_bind
                } else {
                    &self.atlas_bind
                };
                pass.set_bind_group(1, atlas, &[]);
                pass.set_vertex_buffer(0, self.item3d_vbuf.slice(..));
                pass.draw(0..self.item3d_vertex_count, 0..1);
            }
        }
        // CROSSHAIR PASS: the center invert-blend crosshair. Color Load, NO depth.
        if self.crosshair_vertex_count > 0 {
            let mut pass = color_depth_pass(
                &mut enc,
                &view,
                &self.depth,
                "crosshair pass",
                wgpu::LoadOp::Load,
                None,
            );
            pass.set_pipeline(&self.crosshair_pipe);
            pass.set_vertex_buffer(0, self.crosshair_vbuf.slice(..));
            pass.draw(0..self.crosshair_vertex_count, 0..1);
        }
        // UI PASS: dim backdrop → baked panel → dynamic overlays → hover highlight
        // → per-slot item icons, all via `ui_pipe` (own alpha blend, NO depth). Each
        // group binds its own texture; solid quads bind the icon atlas (the solid
        // sentinel skips the sampler, so any layout-compatible texture works).
        let kind = self.ui_build.kind;
        if self.ui_dim_vertex_count > 0
            || self.ui_panel_vertex_count > 0
            || self.ui_overlay_vertex_count > 0
            || self.ui_hover_vertex_count > 0
            || self.icon_quad_vertex_count > 0
        {
            let mut pass = color_depth_pass(&mut enc, &view, &self.depth, "ui pass", wgpu::LoadOp::Load, None);
            pass.set_pipeline(&self.ui_pipe);
            // 1) Dim backdrop behind an open menu.
            if self.ui_dim_vertex_count > 0 {
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui_solid_vbuf.slice(..));
                pass.draw(0..self.ui_dim_vertex_count, 0..1);
            }
            // 2) Baked panel PNG for the open GUI.
            if self.ui_panel_vertex_count > 0 {
                if let Some(bind) = kind.and_then(|k| self.gui_textures.get(&GuiTexId::Panel(k))) {
                    pass.set_bind_group(0, bind, &[]);
                    pass.set_vertex_buffer(0, self.ui_panel_vbuf.slice(..));
                    pass.draw(0..self.ui_panel_vertex_count, 0..1);
                }
            }
            // 3) Dynamic overlays (furnace gauges): one draw per tagged span, each
            //    bound to its own overlay texture.
            if self.ui_overlay_vertex_count > 0 {
                pass.set_vertex_buffer(0, self.ui_overlay_vbuf.slice(..));
                let mut start = 0u32;
                for span in &self.ui_build.overlay_spans {
                    let end = start + span.count;
                    if let Some(bind) = kind.and_then(|k| self.gui_textures.get(&GuiTexId::Overlay(k, span.tag))) {
                        pass.set_bind_group(0, bind, &[]);
                        pass.draw(start..end, 0..1);
                    }
                    start = end;
                }
            }
            // 4) Hover / selection highlight, over the panel, under the icons.
            if self.ui_hover_vertex_count > 0 {
                if let Some(bind) = kind.and_then(|k| self.gui_textures.get(&GuiTexId::Hover(k))) {
                    pass.set_bind_group(0, bind, &[]);
                    pass.set_vertex_buffer(0, self.ui_hover_vbuf.slice(..));
                    pass.draw(0..self.ui_hover_vertex_count, 0..1);
                }
            }
            // 5) Per-slot item icons (icon atlas), one bind + one draw.
            if self.icon_quad_vertex_count > 0 {
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.icon_quad_vbuf.slice(..));
                pass.draw(0..self.icon_quad_vertex_count, 0..1);
            }
        }
        // UI OVERLAY / DRAG PASS: stack counts, then the cursor-held icon, then its
        // count — keeping the whole dragged stack front-most.
        if self.ui_count_vertex_count > 0
            || self.drag_icon_quad_vertex_count > 0
            || self.ui_drag_count_vertex_count > 0
        {
            let mut pass = color_depth_pass(&mut enc, &view, &self.depth, "ui overlay / drag pass", wgpu::LoadOp::Load, None);
            pass.set_pipeline(&self.ui_pipe);
            // Normal stack counts (solid), packed after the dim backdrop.
            if self.ui_count_vertex_count > 0 {
                let start = self.ui_dim_vertex_count;
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui_solid_vbuf.slice(..));
                pass.draw(start..start + self.ui_count_vertex_count, 0..1);
            }
            // Cursor-held icon, appended after the normal icons.
            if self.drag_icon_quad_vertex_count > 0 {
                let start = self.icon_quad_vertex_count;
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.icon_quad_vbuf.slice(..));
                pass.draw(start..start + self.drag_icon_quad_vertex_count, 0..1);
            }
            // Cursor-held count (solid), packed after the normal counts.
            if self.ui_drag_count_vertex_count > 0 {
                let start = self.ui_dim_vertex_count + self.ui_count_vertex_count;
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui_solid_vbuf.slice(..));
                pass.draw(start..start + self.ui_drag_count_vertex_count, 0..1);
            }
        }
        self.queue.submit(std::iter::once(enc.finish()));
        self.last_stats = stats;
        frame.present();
    }
}
