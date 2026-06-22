use crate::camera::{Camera, Frustum};
use crate::chunk::{ChunkPos, CHUNK_SY, SECTION_COUNT, SECTION_SIZE};
use crate::mathh::SelectionShape;
use crate::mesh::MeshIndexSection;
use crate::world::World;

use std::collections::HashMap;
use wgpu::util::DeviceExt;

use super::block_model::BillboardBasis;
use super::break_overlay::build_break_overlay;
use super::crosshair::crosshair_vertices;
use super::hand::{build_hand_lit, HeldItemAnimator};
use super::item_entity::build_item_entities;
use super::particles::build_particles;
use super::pipeline::create_pipeline_resources;
use super::resources::{create_atlas, create_depth, create_gui_atlas, upload_mesh, GpuMesh};
use super::section_cull::SectionVisibilityCache;
use super::selection::outline_vertices;
use super::ui::{build_ui, UiBuild, UiVertex};
use super::uniforms::{Uniforms, FOG_END, FOG_START, UNDERWATER_FOG_END, UNDERWATER_FOG_START};
use super::{
    BreakOverlayView, HeldItemFrame, HeldItemView, ItemEntityInstance, ParticleInstance, UiFrame,
};
use crate::inventory::TOTAL_SLOTS;
use crate::item::ItemType;

const FAR_LEAF_LOD_FADE_START: f32 = 128.0;
const FAR_LEAF_LOD_FADE_END: f32 = 192.0;
const MIN_SECTION_CULL_INDEX_SAVINGS: u32 = 2_048;

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
    /// Which crafting layout the open panel shows (only meaningful when `open`).
    pub panel: super::ui::CraftKind,
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
}

impl Default for UiSnapshot {
    fn default() -> Self {
        UiSnapshot {
            open: false,
            panel: super::ui::CraftKind::Inventory,
            screen: (0, 0),
            cursor_px: (0.0, 0.0),
            active: 0,
            slots: [None; TOTAL_SLOTS],
            craft: [None; crate::crafting::MAX_CELLS],
            result: None,
            cursor: None,
        }
    }
}

pub struct Renderer {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub atlas_texture: wgpu::Texture,
    pub atlas_view: wgpu::TextureView,
    pub atlas_sampler: wgpu::Sampler,
    /// Separate GUI sprite atlas (NOT the block atlas) for the UI pass; built
    /// once in `new_renderer_inner`. See `resources::create_gui_atlas`.
    pub gui_texture: wgpu::Texture,
    pub gui_view: wgpu::TextureView,
    pub gui_sampler: wgpu::Sampler,
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
    /// model3d pipeline (iso slot icons, depthless UI pass): per-draw MVP via a
    /// dynamic-offset uniform + the block atlas, full-bright, no depth.
    pub model3d_pipe: wgpu::RenderPipeline,
    /// Depth-enabled model3d variant for the first-person held block in the hand
    /// pass (same shader; the hand pass clears depth so the held block self-sorts).
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
    /// Index count of the hand geometry uploaded for this frame (0 = nothing).
    hand_index_count: u32,
    /// Vertex count of the hand geometry (icons are appended after it in the
    /// shared model3d vbuf, so their `base_vertex` starts here).
    hand_vertex_count: u32,
    /// Reusable CPU staging for the per-frame hand geometry (cleared + refilled by
    /// `build_hand`, capacity retained — no per-frame allocation).
    hand_verts: Vec<crate::mesh::Vertex>,
    hand_indices: Vec<u32>,
    /// Break-overlay (destroy crack) pipeline + its reusable dynamic buffers.
    pub break_pipe: wgpu::RenderPipeline,
    pub break_vbuf: wgpu::Buffer,
    pub break_ibuf: wgpu::Buffer,
    /// Index count of the break-overlay geometry this frame (0 = no overlay).
    break_index_count: u32,
    /// Item-entity dynamic buffers (drawn by the opaque pipeline).
    pub item_entity_vbuf: wgpu::Buffer,
    pub item_entity_ibuf: wgpu::Buffer,
    /// Particle billboard pipeline + its reusable vbuf and static quad ibuf.
    pub particle_pipe: wgpu::RenderPipeline,
    pub particle_vbuf: wgpu::Buffer,
    pub particle_ibuf: wgpu::Buffer,
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
    /// Dropped item-entities to draw in the world this frame.
    pub item_entities: Vec<ItemEntityInstance>,
    /// Particle billboards to draw this frame.
    pub particles: Vec<ParticleInstance>,
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
    /// Index count of the item-entity geometry uploaded for this frame (0 = none).
    item_entity_index_count: u32,
    /// Reusable CPU staging for baked particle vertices.
    particle_verts: Vec<super::particles::ParticleVertex>,
    /// Vertex count of the particle geometry uploaded for this frame (0 = none).
    particle_vertex_count: u32,
    /// UI pipeline (2D HUD / inventory), its gui-atlas bind, and the reusable
    /// dynamic vbuf for UI quads.
    pub ui_pipe: wgpu::RenderPipeline,
    pub ui_bind: wgpu::BindGroup,
    pub ui_vbuf: wgpu::Buffer,
    /// Reusable CPU staging for the per-frame UI geometry (gui quads + per-slot
    /// icon draws + digit overlay quads), cleared + refilled each frame.
    ui_build: UiBuild,
    /// Vertex count of the gui-quad background uploaded this frame (offset 0).
    ui_bg_vertex_count: u32,
    /// Vertex count of the digit-overlay quads uploaded this frame (after the bg).
    ui_overlay_vertex_count: u32,
    /// Per-slot icon draws for the UI pass this frame. Each references a 256-aligned
    /// MVP slot in `model3d_mvp_buf` and an index range in the shared model3d ibuf
    /// (appended after the hand geometry, so `base_vertex` starts past the hand).
    ui_icons: Vec<UiIconDraw>,
}

/// A baked UI icon's draw parameters for the UI pass: which model3d MVP slot holds
/// its transform, and where its geometry lives in the shared model3d index/vertex
/// buffers (appended after the hand each frame).
#[derive(Copy, Clone, Debug)]
struct UiIconDraw {
    /// 256-byte dynamic offset into `model3d_mvp_buf` (slot index × 256).
    mvp_offset: u32,
    index_start: u32,
    index_count: u32,
    base_vertex: i32,
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

/// Instance descriptor selecting all native backends (Vulkan/Metal/DX12/GL).
pub fn instance_descriptor() -> wgpu::InstanceDescriptor {
    wgpu::InstanceDescriptor::default()
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
    let (gui_texture, gui_view, gui_sampler) = create_gui_atlas(&device, &queue);
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
        &gui_view,
        &gui_sampler,
    );
    let depth = create_depth(&device, width, height);

    Renderer {
        surface,
        device,
        queue,
        config,
        atlas_texture,
        atlas_view,
        atlas_sampler,
        gui_texture,
        gui_view,
        gui_sampler,
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
        model3d_pipe: pipelines.model3d_pipe,
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
        hand_index_count: 0,
        hand_verts: Vec::new(),
        hand_indices: Vec::new(),
        break_pipe: pipelines.break_pipe,
        break_vbuf: pipelines.break_vbuf,
        break_ibuf: pipelines.break_ibuf,
        break_index_count: 0,
        item_entity_vbuf: pipelines.item_entity_vbuf,
        item_entity_ibuf: pipelines.item_entity_ibuf,
        particle_pipe: pipelines.particle_pipe,
        particle_vbuf: pipelines.particle_vbuf,
        particle_ibuf: pipelines.particle_ibuf,
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
        item_entities: Vec::new(),
        particles: Vec::new(),
        ui: UiSnapshot::default(),
        billboard_basis: BillboardBasis {
            right: glam::Vec3::X,
            up: glam::Vec3::Y,
        },
        item_entity_verts: Vec::new(),
        item_entity_indices: Vec::new(),
        item_entity_visible: Vec::new(),
        item_entity_index_count: 0,
        particle_verts: Vec::new(),
        particle_vertex_count: 0,
        ui_pipe: pipelines.ui_pipe,
        ui_bind: pipelines.ui_bind,
        ui_vbuf: pipelines.ui_vbuf,
        ui_build: UiBuild {
            verts: Vec::new(),
            icons: Vec::new(),
            icon_verts: Vec::new(),
            icon_indices: Vec::new(),
            overlay_verts: Vec::new(),
        },
        ui_bg_vertex_count: 0,
        ui_overlay_vertex_count: 0,
        ui_icons: Vec::new(),
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

    /// Store the world skylight to apply to the first-person hand / held item.
    pub fn set_held_item_light(&mut self, skylight: u8) {
        self.held_item_skylight = skylight.min(super::lighting::FULL_SKYLIGHT);
    }

    /// Store the dropped item-entities to draw this frame. Reuses the existing
    /// `Vec` capacity (clear + extend) to avoid per-frame reallocation.
    pub fn set_item_entities(&mut self, v: &[ItemEntityInstance]) {
        self.item_entities.clear();
        self.item_entities.extend_from_slice(v);
    }

    /// Store the particle billboards to draw this frame. Reuses capacity.
    pub fn set_particles(&mut self, v: &[ParticleInstance]) {
        self.particles.clear();
        self.particles.extend_from_slice(v);
    }

    /// Snapshot the UI/inventory bits needed for this frame's UI pass. Extracts
    /// the small flat state (open flag, screen, cursor, 36 slot stacks, active
    /// slot, cursor stack) into owned `Renderer` state so the renderer never
    /// holds a borrow of the game `Inventory`.
    pub fn set_ui(&mut self, v: UiFrame) {
        self.ui.open = v.open;
        self.ui.panel = v.panel;
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
        // Drop GPU meshes whose CPU chunk is gone — checked directly against the
        // world's mesh map so no per-frame scratch set is allocated.
        self.chunk_meshes
            .retain(|p, _| world.meshes.contains_key(p));
        // Upload only meshes marked dirty by the world (newly built/changed) or
        // missing on the GPU; clear each CPU dirty flag as it is uploaded. Existing
        // unchanged meshes are left on the GPU untouched.
        for (pos, mesh) in world.meshes.iter_mut() {
            let need_upload = !self.chunk_meshes.contains_key(pos) || mesh.mesh_dirty;
            if need_upload {
                let gm = upload_mesh(&self.device, mesh, *pos);
                self.chunk_meshes.insert(*pos, gm);
                mesh.mesh_dirty = false;
            }
        }
    }

    pub fn update_section_visibility(&mut self, world: &mut World) {
        self.section_visibility.update(world, self.cam_pos);
    }

    /// Build + upload this frame's UI geometry. Called from `render` after the hand
    /// is uploaded (icons are appended into the SAME model3d vbuf/ibuf after the
    /// hand). Fills:
    /// - `ui_vbuf`: gui-atlas quads — the background sprites/fills at offset 0
    ///   (`ui_bg_vertex_count` verts) then the digit-overlay quads right after
    ///   (`ui_overlay_vertex_count` verts).
    /// - `model3d_vbuf` / `model3d_ibuf`: per-slot icon geometry appended past the
    ///   hand (`hand_vertex_count` / `hand_index_count`).
    /// - `model3d_mvp_buf`: each icon's MVP in its own 256-aligned slot (1..N).
    fn build_ui_frame(&mut self) {
        self.ui_bg_vertex_count = 0;
        self.ui_overlay_vertex_count = 0;
        self.ui_icons.clear();

        // Disjoint-field borrow: `build_ui` reads the snapshot and writes the
        // scratch `UiBuild`, both distinct from the GPU buffers used below.
        build_ui(&self.ui, &mut self.ui_build);

        // gui-atlas quads: background first (offset 0), then digit overlay.
        let bg = &self.ui_build.verts;
        let overlay = &self.ui_build.overlay_verts;
        let cap = super::pipeline::MAX_UI_VERTICES as usize;
        if !bg.is_empty() && bg.len() <= cap {
            self.queue
                .write_buffer(&self.ui_vbuf, 0, bytemuck::cast_slice(bg));
            self.ui_bg_vertex_count = bg.len() as u32;
        }
        if !overlay.is_empty() && self.ui_bg_vertex_count as usize + overlay.len() <= cap {
            let byte_off =
                (self.ui_bg_vertex_count as usize * std::mem::size_of::<UiVertex>()) as u64;
            self.queue
                .write_buffer(&self.ui_vbuf, byte_off, bytemuck::cast_slice(overlay));
            self.ui_overlay_vertex_count = overlay.len() as u32;
        }

        // Per-slot icons: ALL icon geometry sits in the shared, reused
        // `ui_build.icon_verts`/`icon_indices` (cleared + refilled by `build_ui`,
        // no per-icon allocation). The indices are global within `icon_verts`, so
        // the whole accepted prefix is one contiguous block appended after the hand
        // in the model3d vbuf/ibuf; each icon draws its index sub-range with a
        // shared `base_vertex` (the offset of the icon block past the hand).
        let vstride = std::mem::size_of::<crate::mesh::Vertex>() as u64;
        let vcap = super::pipeline::MAX_MODEL3D_VERTICES;
        let icap = super::pipeline::MAX_MODEL3D_INDICES;
        let slot_size = super::pipeline::MODEL3D_MVP_SLOT_SIZE;
        let max_slots = super::pipeline::MODEL3D_MVP_SLOTS;
        let vbase = self.hand_vertex_count as u64; // verts already used by the hand
        let ibase = self.hand_index_count as u64; // indices already used by the hand
                                                  // Decide which leading icons fit (vbuf/ibuf capacity + MVP slot count).
                                                  // `icon_verts`/`icon_indices` are filled in icon order, so the fitting set
                                                  // is always a prefix and its geometry is a contiguous slice.
        let mut fit = 0usize;
        for (i, icon) in self.ui_build.icons.iter().enumerate() {
            let slot = (i + 1) as u64; // MVP slot 0 is the hand; icons use 1..
            if slot >= max_slots {
                break;
            }
            let v_end = vbase + (icon.vert_start + icon.vert_count) as u64;
            let i_end = ibase + (icon.index_start + icon.index_count) as u64;
            if v_end > vcap || i_end > icap {
                break;
            }
            fit = i + 1;
        }
        if fit > 0 {
            let last = &self.ui_build.icons[fit - 1];
            let nv = (last.vert_start + last.vert_count) as usize;
            let ni = (last.index_start + last.index_count) as usize;
            // One batch upload of the accepted icon-geometry prefix past the hand.
            self.queue.write_buffer(
                &self.model3d_vbuf,
                vbase * vstride,
                bytemuck::cast_slice(&self.ui_build.icon_verts[..nv]),
            );
            self.queue.write_buffer(
                &self.model3d_ibuf,
                ibase * 4,
                bytemuck::cast_slice(&self.ui_build.icon_indices[..ni]),
            );
            for (i, icon) in self.ui_build.icons[..fit].iter().enumerate() {
                let slot = (i + 1) as u64;
                self.queue.write_buffer(
                    &self.model3d_mvp_buf,
                    slot * slot_size,
                    bytemuck::cast_slice(&icon.mvp.to_cols_array()),
                );
                self.ui_icons.push(UiIconDraw {
                    mvp_offset: (slot * slot_size) as u32,
                    index_start: ibase as u32 + icon.index_start,
                    index_count: icon.index_count,
                    base_vertex: vbase as i32,
                });
            }
        }
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
        {
            let aspect = if self.config.height > 0 {
                self.config.width as f32 / self.config.height as f32
            } else {
                1.0
            };
            if let Some((tile, mvp)) = super::hand::held_sprite(&self.held_item, aspect) {
                let mut iv = std::mem::take(&mut self.item3d_verts);
                let count = super::item_model::build_extruded_item_lit(
                    tile,
                    self.held_item_skylight,
                    &mut iv,
                );
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
        // digit overlay) into the UI vbuf, and the per-slot item icons appended
        // into the SHARED model3d vbuf/ibuf AFTER the hand geometry. Each icon's
        // MVP is written into its own 256-aligned slot of `model3d_mvp_buf`
        // (slot 0 is the hand; icons take slots 1..). All buffers are rewritten in
        // place — capacity is retained across frames.
        self.build_ui_frame();

        // Build + upload the item-entity geometry (spinning cubes / sprite
        // billboards) into the reusable model-format buffers. Frustum-culled
        // against the camera so off-screen drops cost nothing. Drawn by the EXISTING
        // opaque pipeline (no new pipeline) in the item-entity pass below.
        self.item_entity_index_count = 0;
        if !self.item_entities.is_empty() {
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
            let count = build_item_entities(
                &self.item_entity_visible,
                self.billboard_basis,
                &mut self.item_entity_verts,
                &mut self.item_entity_indices,
            );
            // Item entities use their own fixed-size dynamic buffers (not the
            // model3d/hand buffers, which the hand pass writes separately). Bail if
            // the bake would overflow the budget so the buffers stay fixed-size.
            let vbuf_cap = super::pipeline::MAX_ITEM_ENTITY_VERTICES as usize;
            let ibuf_cap = super::pipeline::MAX_ITEM_ENTITY_INDICES as usize;
            if count > 0
                && self.item_entity_verts.len() <= vbuf_cap
                && self.item_entity_indices.len() <= ibuf_cap
            {
                self.queue.write_buffer(
                    &self.item_entity_vbuf,
                    0,
                    bytemuck::cast_slice(&self.item_entity_verts),
                );
                self.queue.write_buffer(
                    &self.item_entity_ibuf,
                    0,
                    bytemuck::cast_slice(&self.item_entity_indices),
                );
                self.item_entity_index_count = count;
            }
        }

        // Build + upload the break-overlay (destroy crack) cube, when targeted.
        self.break_index_count = 0;
        if let Some(view) = self.break_overlay {
            let count = build_break_overlay(
                &view,
                &mut self.item_entity_verts,
                &mut self.item_entity_indices,
            );
            if count > 0 {
                self.queue.write_buffer(
                    &self.break_vbuf,
                    0,
                    bytemuck::cast_slice(&self.item_entity_verts),
                );
                self.queue.write_buffer(
                    &self.break_ibuf,
                    0,
                    bytemuck::cast_slice(&self.item_entity_indices),
                );
                self.break_index_count = count;
            }
        }

        // Build + upload tiny 3D particle cubes into the reusable vbuf.
        self.particle_vertex_count = 0;
        if !self.particles.is_empty() {
            let count = build_particles(&self.particles, &mut self.particle_verts);
            if count > 0 {
                self.queue.write_buffer(
                    &self.particle_vbuf,
                    0,
                    bytemuck::cast_slice(&self.particle_verts),
                );
                self.particle_vertex_count = count;
            }
        }

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
        for gm in self.chunk_meshes.values() {
            if !self.chunk_visible(gm) {
                continue;
            }
            frustum_chunks += 1;
            if section_culling_active && self.section_visibility.chunk_mask(gm.pos).is_none() {
                continue;
            }
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
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: cc[0] as f64,
                            g: cc[1] as f64,
                            b: cc[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.sky_pipe);
            pass.set_bind_group(0, &self.sky_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("opaque pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
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
        // ITEM-ENTITY PASS (§8 2b): dropped items as full-bright spinning cubes /
        // sprite billboards, drawn by the EXISTING opaque pipeline (no new
        // pipeline) with the SAME uniform + atlas binds. Load color + depth,
        // depth test + write so items occlude and are occluded by terrain.
        if self.item_entity_index_count > 0 {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("item entity pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.opaque_pipe);
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_vertex_buffer(0, self.item_entity_vbuf.slice(..));
            pass.set_index_buffer(self.item_entity_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.item_entity_index_count, 0, 0..1);
        }
        // BREAK-OVERLAY PASS: the destroy crack over the targeted block. Drawn
        // BEFORE the transparent water pass — it is a decal on the OPAQUE block, so
        // water must be able to blend in front of it (a crack on a submerged block
        // shows THROUGH the water, not over it). MULTIPLY blend; depth LessEqual /
        // no-write over a cube built COINCIDENT with the block faces (no inflation,
        // so the decal never misaligns), with a small polygon offset toward the
        // camera (BREAK_DEPTH_BIAS) so it wins the depth tie cleanly. Reuses
        // uniform_bind (view_proj + uv_rects) + atlas_bind.
        if self.break_index_count > 0 {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("break overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.break_pipe);
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_vertex_buffer(0, self.break_vbuf.slice(..));
            pass.set_index_buffer(self.break_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.break_index_count, 0, 0..1);
        }
        // PARTICLE PASS (§8 3b): tiny 3D terrain particle cubes. Drawn BEFORE the
        // transparent water pass (but after the break overlay, so they sit in front
        // of the crack): they are alpha-CUTOUT solids that DEPTH-TEST + DEPTH-WRITE,
        // so water blends over the ones behind it (underwater dust reads as
        // submerged) while ones in front of the water still occlude it. Reuses
        // uniform_bind + atlas_bind. 24 verts / 36 indices per cube.
        if self.particle_vertex_count > 0 {
            let index_count = self.particle_vertex_count / super::particles::VERTS_PER_CUBE as u32
                * super::particles::INDICES_PER_CUBE as u32;
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("particle pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.particle_pipe);
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_vertex_buffer(0, self.particle_vbuf.slice(..));
            pass.set_index_buffer(self.particle_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..index_count, 0, 0..1);
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("transparent pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
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
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
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
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hand pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // Bare arm / held block (model3d, depth-enabled hand variant).
            if self.hand_index_count > 0 {
                pass.set_pipeline(&self.model3d_hand_pipe);
                pass.set_bind_group(0, &self.model3d_mvp_bind, &[0]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.model3d_vbuf.slice(..));
                pass.set_index_buffer(self.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.hand_index_count, 0, 0..1);
            }
            // Extruded held sprite (item3d, non-indexed triangle list).
            if self.item3d_vertex_count > 0 {
                pass.set_pipeline(&self.item3d_pipe);
                pass.set_bind_group(0, &self.item3d_mvp_bind, &[0]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.item3d_vbuf.slice(..));
                pass.draw(0..self.item3d_vertex_count, 0..1);
            }
        }
        if self.crosshair_vertex_count > 0 {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("crosshair pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.crosshair_pipe);
            pass.set_vertex_buffer(0, self.crosshair_vbuf.slice(..));
            pass.draw(0..self.crosshair_vertex_count, 0..1);
        }
        // UI PASS (§8 5b): the LAST pass — hotbar / open inventory / slot icons /
        // digits / drag cursor. Its OWN alpha blend, NO depth; it must not inherit
        // the crosshair invert blend (separate pipeline + render pass). Within the
        // one pass we interleave: gui-atlas background quads (ui_pipe), then the
        // per-slot 3D item icons (model3d_pipe, painting over the slots), then the
        // gui-atlas digit overlay (ui_pipe, painting over the icons).
        if self.ui_bg_vertex_count > 0 || !self.ui_icons.is_empty() {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // 1) gui-atlas background (hotbar / panel / selection / dim).
            if self.ui_bg_vertex_count > 0 {
                pass.set_pipeline(&self.ui_pipe);
                pass.set_bind_group(0, &self.ui_bind, &[]);
                pass.set_vertex_buffer(0, self.ui_vbuf.slice(..));
                pass.draw(0..self.ui_bg_vertex_count, 0..1);
            }
            // 2) per-slot 3D item icons (model3d pipeline + block atlas + per-icon
            //    dynamic-offset MVP). Painted over the gui background.
            if !self.ui_icons.is_empty() {
                pass.set_pipeline(&self.model3d_pipe);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.model3d_vbuf.slice(..));
                pass.set_index_buffer(self.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                for icon in &self.ui_icons {
                    pass.set_bind_group(0, &self.model3d_mvp_bind, &[icon.mvp_offset]);
                    pass.draw_indexed(
                        icon.index_start..icon.index_start + icon.index_count,
                        icon.base_vertex,
                        0..1,
                    );
                }
            }
            // 3) gui-atlas digit overlay (counts + drag-count), over the icons.
            if self.ui_overlay_vertex_count > 0 {
                pass.set_pipeline(&self.ui_pipe);
                pass.set_bind_group(0, &self.ui_bind, &[]);
                pass.set_vertex_buffer(0, self.ui_vbuf.slice(..));
                pass.draw(
                    self.ui_bg_vertex_count..self.ui_bg_vertex_count + self.ui_overlay_vertex_count,
                    0..1,
                );
            }
        }
        self.queue.submit(std::iter::once(enc.finish()));
        self.last_stats = stats;
        frame.present();
    }
}

struct SectionDrawRanges {
    ranges: [(u32, u32); SECTION_COUNT],
    len: usize,
    submitted: u32,
}

impl SectionDrawRanges {
    fn new() -> Self {
        Self {
            ranges: [(0, 0); SECTION_COUNT],
            len: 0,
            submitted: 0,
        }
    }

    fn full(index_count: u32) -> Self {
        let mut out = Self::new();
        if index_count > 0 {
            out.ranges[0] = (0, index_count);
            out.len = 1;
            out.submitted = index_count;
        }
        out
    }

    fn push(&mut self, start: u32, end: u32) {
        if start >= end {
            return;
        }
        if self.len > 0 && self.ranges[self.len - 1].1 == start {
            self.ranges[self.len - 1].1 = end;
        } else {
            self.ranges[self.len] = (start, end);
            self.len += 1;
        }
        self.submitted += end - start;
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.ranges[..self.len].iter().copied()
    }
}

fn section_draw_ranges(
    frustum: Frustum,
    origin: (i32, i32),
    full_idx_count: u32,
    sections: &[MeshIndexSection; SECTION_COUNT],
    visible_mask: u16,
) -> SectionDrawRanges {
    let mut out = SectionDrawRanges::new();
    for (section_idx, section) in sections.iter().enumerate() {
        if visible_mask & (1u16 << section_idx) == 0 || section.index_count == 0 {
            continue;
        }
        if !section_visible(frustum, origin, section_idx) {
            continue;
        }
        out.push(
            section.first_index,
            section.first_index + section.index_count,
        );
    }

    if out.is_empty() || out.submitted >= full_idx_count {
        return out;
    }
    if out.len == 1 {
        return out;
    }

    let saved = full_idx_count - out.submitted;
    let saves_enough_indices = saved >= MIN_SECTION_CULL_INDEX_SAVINGS;
    let saves_enough_ratio = (out.submitted as u64) * 4 <= (full_idx_count as u64) * 3;
    if saves_enough_indices && saves_enough_ratio {
        out
    } else {
        SectionDrawRanges::full(full_idx_count)
    }
}

fn section_visible(frustum: Frustum, origin: (i32, i32), section_idx: usize) -> bool {
    let (ox, oz) = origin;
    let y0 = (section_idx * SECTION_SIZE) as f32;
    let y1 = ((section_idx + 1) * SECTION_SIZE).min(CHUNK_SY) as f32;
    let min = glam::Vec3::new(ox as f32, y0, oz as f32);
    let max = glam::Vec3::new((ox + 16) as f32, y1, (oz + 16) as f32);
    frustum.aabb_visible(min, max)
}

fn far_leaf_lod_active(dist_sq: f32, origin: (i32, i32), has_far_lod: bool) -> bool {
    if !has_far_lod {
        return false;
    }

    let dist = dist_sq.sqrt();
    if dist <= FAR_LEAF_LOD_FADE_START {
        return false;
    }
    if dist >= FAR_LEAF_LOD_FADE_END {
        return true;
    }

    let t = (dist - FAR_LEAF_LOD_FADE_START) / (FAR_LEAF_LOD_FADE_END - FAR_LEAF_LOD_FADE_START);
    let smooth = t * t * (3.0 - 2.0 * t);
    smooth >= chunk_lod_threshold(origin)
}

fn chunk_lod_threshold(origin: (i32, i32)) -> f32 {
    let mut h =
        (origin.0 as u32).wrapping_mul(0x9E37_79B1) ^ (origin.1 as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    ((h & 0xFFFF) as f32 + 0.5) / 65_536.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_draw_ranges_keep_single_visible_section() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[2] = MeshIndexSection {
            first_index: 120,
            index_count: 60,
        };

        let ranges = section_draw_ranges(frustum, (0, 0), 480, &sections, 1u16 << 2);

        assert_eq!(ranges.iter().collect::<Vec<_>>(), vec![(120, 180)]);
        assert_eq!(ranges.submitted, 60);
    }

    #[test]
    fn section_draw_ranges_fall_back_when_fragmented_savings_are_small() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[0] = MeshIndexSection {
            first_index: 0,
            index_count: 100,
        };
        sections[2] = MeshIndexSection {
            first_index: 200,
            index_count: 100,
        };

        let ranges = section_draw_ranges(frustum, (0, 0), 360, &sections, 0b0101);

        assert_eq!(ranges.iter().collect::<Vec<_>>(), vec![(0, 360)]);
        assert_eq!(ranges.submitted, 360);
    }

    #[test]
    fn section_draw_ranges_keep_fragmented_ranges_when_savings_are_large() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[0] = MeshIndexSection {
            first_index: 0,
            index_count: 600,
        };
        sections[8] = MeshIndexSection {
            first_index: 8_000,
            index_count: 600,
        };

        let ranges = section_draw_ranges(
            frustum,
            (0, 0),
            12_000,
            &sections,
            (1u16 << 0) | (1u16 << 8),
        );

        assert_eq!(
            ranges.iter().collect::<Vec<_>>(),
            vec![(0, 600), (8_000, 8_600)]
        );
        assert_eq!(ranges.submitted, 1_200);
    }

    #[test]
    fn far_leaf_lod_stays_near_and_converges_far() {
        assert!(!far_leaf_lod_active(200.0 * 200.0, (0, 0), false));
        assert!(!far_leaf_lod_active(
            FAR_LEAF_LOD_FADE_START * FAR_LEAF_LOD_FADE_START,
            (0, 0),
            true
        ));
        assert!(far_leaf_lod_active(
            FAR_LEAF_LOD_FADE_END * FAR_LEAF_LOD_FADE_END,
            (0, 0),
            true
        ));
    }

    #[test]
    fn far_leaf_lod_transition_is_staggered_by_chunk() {
        let mid = ((FAR_LEAF_LOD_FADE_START + FAR_LEAF_LOD_FADE_END) * 0.5).powi(2);
        let mut near_count = 0;
        let mut far_count = 0;
        for z in -8..=8 {
            for x in -8..=8 {
                if far_leaf_lod_active(mid, (x * 16, z * 16), true) {
                    far_count += 1;
                } else {
                    near_count += 1;
                }
            }
        }

        assert!(near_count > 0);
        assert!(far_count > 0);
    }
}
